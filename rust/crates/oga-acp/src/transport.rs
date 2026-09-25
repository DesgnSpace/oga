//! Newline-delimited JSON-RPC over a child's pipes.

use std::{
    collections::HashMap,
    io,
    marker::PhantomData,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering},
    },
    time::Duration,
};

use agent_client_protocol_schema::v1::Error;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use thiserror::Error as ThisError;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::{mpsc, oneshot},
    task::JoinHandle,
    time::timeout,
};

use crate::policy::PolicyFuture;

pub const DEFAULT_MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;
pub const DEFAULT_MAX_STDERR_BYTES: usize = 64 * 1024;

#[derive(Debug, ThisError)]
pub(crate) enum RpcError {
    #[error("the agent was no longer connected when {method} was queued")]
    NotSent { method: String },
    #[error("the agent closed the connection")]
    Closed,
    #[error("the agent did not answer {method} within {}ms", timeout.as_millis())]
    Timeout { method: String, timeout: Duration },
    #[error("the agent answered {method} with an error: {message}")]
    Agent {
        method: String,
        code: i32,
        message: String,
        data: Option<Value>,
    },
    #[error("could not read the agent's answer to {method}: {source}")]
    Decode {
        method: String,
        #[source]
        source: serde_json::Error,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Diagnostics {
    pub malformed_frames: usize,
    pub oversized_frames: usize,
    pub dropped_bytes: usize,
    pub stderr: String,
    pub stderr_truncated: bool,
}

pub(crate) trait Handler: Send + Sync + 'static {
    fn request(
        self: Arc<Self>,
        method: String,
        params: Value,
    ) -> PolicyFuture<'static, Result<Value, Error>>;

    fn notification(self: Arc<Self>, method: String, params: Value);
}

type Pending = Mutex<HashMap<i64, oneshot::Sender<Result<Value, Error>>>>;

#[derive(Debug, Default)]
struct Shared {
    pending: Pending,
    closed: AtomicBool,
    malformed_frames: AtomicUsize,
    oversized_frames: AtomicUsize,
    dropped_bytes: AtomicUsize,
}

impl Shared {
    fn drop_frame(&self, counter: &AtomicUsize, bytes: usize) {
        counter.fetch_add(1, Ordering::Relaxed);
        self.dropped_bytes.fetch_add(bytes, Ordering::Relaxed);
    }

    fn take_waiter(&self, id: i64) -> Option<oneshot::Sender<Result<Value, Error>>> {
        self.pending
            .lock()
            .expect("pending lock is not poisoned")
            .remove(&id)
    }
}

#[derive(Debug)]
struct StderrTail {
    text: Mutex<Vec<u8>>,
    truncated: AtomicBool,
    limit: usize,
}

impl StderrTail {
    fn push(&self, chunk: &[u8]) {
        let mut text = self.text.lock().expect("stderr lock is not poisoned");
        text.extend_from_slice(chunk);
        if text.len() > self.limit {
            let excess = text.len() - self.limit;
            text.drain(..excess);
            self.truncated.store(true, Ordering::Release);
        }
    }

    fn read(&self) -> (String, bool) {
        let text = self.text.lock().expect("stderr lock is not poisoned");
        (
            String::from_utf8_lossy(&text).into_owned(),
            self.truncated.load(Ordering::Acquire),
        )
    }
}

pub(crate) struct Connection {
    outbound: mpsc::UnboundedSender<Vec<u8>>,
    shared: Arc<Shared>,
    next_id: AtomicI64,
    stderr: Arc<StderrTail>,
    tasks: Vec<JoinHandle<()>>,
}

impl std::fmt::Debug for Connection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Connection")
            .field("closed", &self.is_closed())
            .finish_non_exhaustive()
    }
}

impl Connection {
    pub(crate) fn start<I, O, E, H>(
        stdin: I,
        stdout: O,
        stderr: E,
        handler: Arc<H>,
        max_frame_bytes: usize,
        max_stderr_bytes: usize,
    ) -> Self
    where
        I: AsyncWrite + Send + Unpin + 'static,
        O: AsyncRead + Send + Unpin + 'static,
        E: AsyncRead + Send + Unpin + 'static,
        H: Handler,
    {
        let (outbound, outbound_rx) = mpsc::unbounded_channel();
        let shared = Arc::<Shared>::default();
        let tail = Arc::new(StderrTail {
            text: Mutex::new(Vec::new()),
            truncated: AtomicBool::new(false),
            limit: max_stderr_bytes,
        });

        let writer = tokio::spawn(write_frames(stdin, outbound_rx, Arc::clone(&shared)));
        let reader = tokio::spawn(read_frames(
            stdout,
            max_frame_bytes,
            Arc::clone(&shared),
            handler,
            outbound.clone(),
        ));
        let drain = tokio::spawn(drain_stderr(stderr, Arc::clone(&tail)));

        Self {
            outbound,
            shared,
            next_id: AtomicI64::new(1),
            stderr: tail,
            tasks: vec![writer, reader, drain],
        }
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.shared.closed.load(Ordering::Acquire)
    }

    pub(crate) fn diagnostics(&self) -> Diagnostics {
        let (stderr, stderr_truncated) = self.stderr.read();
        Diagnostics {
            malformed_frames: self.shared.malformed_frames.load(Ordering::Acquire),
            oversized_frames: self.shared.oversized_frames.load(Ordering::Acquire),
            dropped_bytes: self.shared.dropped_bytes.load(Ordering::Acquire),
            stderr,
            stderr_truncated,
        }
    }

    pub(crate) async fn request<P, R>(
        &self,
        method: &str,
        params: &P,
        lifetime: Duration,
    ) -> Result<R, RpcError>
    where
        P: Serialize,
        R: DeserializeOwned,
    {
        self.start_request(method, params)?.answer(lifetime).await
    }

    pub(crate) fn start_request<P, R>(&self, method: &str, params: &P) -> Result<Sent<R>, RpcError>
    where
        P: Serialize,
    {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.shared
            .pending
            .lock()
            .expect("pending lock is not poisoned")
            .insert(id, tx);

        let frame = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        if self.send(&frame).is_err() {
            self.shared.take_waiter(id);
            return Err(RpcError::NotSent {
                method: method.to_owned(),
            });
        }
        Ok(Sent {
            id,
            method: method.to_owned(),
            shared: Arc::clone(&self.shared),
            waiting: rx,
            decoded: PhantomData,
        })
    }

    pub(crate) fn notify<P: Serialize>(&self, method: &str, params: &P) -> Result<(), RpcError> {
        self.send(&json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }))
    }

    fn send(&self, frame: &Value) -> Result<(), RpcError> {
        if self.is_closed() {
            return Err(RpcError::Closed);
        }
        let bytes = serde_json::to_vec(frame).expect("a JSON-RPC frame serializes");
        self.outbound.send(bytes).map_err(|_| RpcError::Closed)
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

pub(crate) struct Sent<R> {
    id: i64,
    method: String,
    shared: Arc<Shared>,
    waiting: oneshot::Receiver<Result<Value, Error>>,
    decoded: PhantomData<R>,
}

impl<R: DeserializeOwned> Sent<R> {
    pub(crate) async fn answer(self, lifetime: Duration) -> Result<R, RpcError> {
        let answer = match timeout(lifetime, self.waiting).await {
            Ok(Ok(answer)) => answer,
            Ok(Err(_)) => return Err(RpcError::Closed),
            Err(_) => {
                self.shared.take_waiter(self.id);
                return Err(RpcError::Timeout {
                    method: self.method,
                    timeout: lifetime,
                });
            }
        };
        match answer {
            Ok(result) => serde_json::from_value(result).map_err(|source| RpcError::Decode {
                method: self.method,
                source,
            }),
            Err(error) => Err(RpcError::Agent {
                method: self.method,
                code: error.code.into(),
                message: error.message,
                data: error.data,
            }),
        }
    }
}

async fn write_frames<I>(
    mut stdin: I,
    mut outbound: mpsc::UnboundedReceiver<Vec<u8>>,
    shared: Arc<Shared>,
) where
    I: AsyncWrite + Send + Unpin + 'static,
{
    while let Some(mut frame) = outbound.recv().await {
        frame.push(b'\n');
        if stdin.write_all(&frame).await.is_err() || stdin.flush().await.is_err() {
            shared.closed.store(true, Ordering::Release);
            return;
        }
    }
    let _ = stdin.shutdown().await;
}

async fn read_frames<O, H>(
    stdout: O,
    max_frame_bytes: usize,
    shared: Arc<Shared>,
    handler: Arc<H>,
    outbound: mpsc::UnboundedSender<Vec<u8>>,
) where
    O: AsyncRead + Send + Unpin + 'static,
    H: Handler,
{
    let mut frames = FrameReader::new(stdout, max_frame_bytes);
    loop {
        match frames.next().await {
            Ok(Some(Frame::Line(line))) => dispatch(&line, &shared, &handler, &outbound),
            Ok(Some(Frame::Oversized(bytes))) => {
                shared.drop_frame(&shared.oversized_frames, bytes);
            }
            Ok(None) | Err(_) => break,
        }
    }
    // Dropping senders tells pending requests the agent is gone instead of making them time out.
    shared.closed.store(true, Ordering::Release);
    shared
        .pending
        .lock()
        .expect("pending lock is not poisoned")
        .clear();
}

fn dispatch<H: Handler>(
    line: &[u8],
    shared: &Arc<Shared>,
    handler: &Arc<H>,
    outbound: &mpsc::UnboundedSender<Vec<u8>>,
) {
    let Ok(message) = serde_json::from_slice::<Value>(line) else {
        shared.drop_frame(&shared.malformed_frames, line.len());
        return;
    };
    let method = message.get("method").and_then(Value::as_str);
    let id = message.get("id");
    match (method, id) {
        (Some(method), Some(id)) => {
            answer_agent(method.to_owned(), id.clone(), &message, handler, outbound);
        }
        (Some(method), None) => {
            let params = message.get("params").cloned().unwrap_or(Value::Null);
            Arc::clone(handler).notification(method.to_owned(), params);
        }
        (None, Some(id)) => resolve(id, &message, shared),
        (None, None) => shared.drop_frame(&shared.malformed_frames, line.len()),
    }
}

fn resolve(id: &Value, message: &Value, shared: &Arc<Shared>) {
    let Some(waiter) = id.as_i64().and_then(|id| shared.take_waiter(id)) else {
        shared.malformed_frames.fetch_add(1, Ordering::Relaxed);
        return;
    };
    let answer = match message.get("error") {
        Some(error) => Err(serde_json::from_value(error.clone())
            .unwrap_or_else(|_| Error::internal_error().data(error.clone()))),
        None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
    };
    let _ = waiter.send(answer);
}

fn answer_agent<H: Handler>(
    method: String,
    id: Value,
    message: &Value,
    handler: &Arc<H>,
    outbound: &mpsc::UnboundedSender<Vec<u8>>,
) {
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    let handler = Arc::clone(handler);
    let outbound = outbound.clone();
    // Policy answers may wait for a person, so they run outside the read loop.
    tokio::spawn(async move {
        let reply = match handler.request(method, params).await {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err(error) => json!({"jsonrpc": "2.0", "id": id, "error": error}),
        };
        let _ = outbound.send(serde_json::to_vec(&reply).expect("a reply serializes"));
    });
}

async fn drain_stderr<E>(mut stderr: E, tail: Arc<StderrTail>)
where
    E: AsyncRead + Send + Unpin + 'static,
{
    let mut chunk = [0_u8; 8 * 1024];
    while let Ok(length) = stderr.read(&mut chunk).await {
        if length == 0 {
            return;
        }
        tail.push(&chunk[..length]);
    }
}

enum Frame {
    Line(Vec<u8>),
    Oversized(usize),
}

struct FrameReader<R> {
    reader: R,
    buffer: Vec<u8>,
    max_frame_bytes: usize,
    discarded: usize,
}

impl<R: AsyncRead + Send + Unpin> FrameReader<R> {
    fn new(reader: R, max_frame_bytes: usize) -> Self {
        Self {
            reader,
            buffer: Vec::new(),
            max_frame_bytes,
            discarded: 0,
        }
    }

    async fn next(&mut self) -> io::Result<Option<Frame>> {
        let mut chunk = [0_u8; 8 * 1024];
        loop {
            if let Some(end) = self.buffer.iter().position(|byte| *byte == b'\n') {
                let line: Vec<u8> = self.buffer.drain(..=end).take(end).collect();
                if self.discarded > 0 {
                    let dropped = self.discarded + line.len();
                    self.discarded = 0;
                    return Ok(Some(Frame::Oversized(dropped)));
                }
                return Ok(Some(Frame::Line(line)));
            }
            if self.buffer.len() > self.max_frame_bytes {
                self.discarded += self.buffer.len();
                self.buffer.clear();
            }
            let length = self.reader.read(&mut chunk).await?;
            if length == 0 {
                if self.discarded > 0 {
                    let dropped = self.discarded + self.buffer.len();
                    self.discarded = 0;
                    self.buffer.clear();
                    return Ok(Some(Frame::Oversized(dropped)));
                }
                if self.buffer.is_empty() {
                    return Ok(None);
                }
                return Ok(Some(Frame::Line(std::mem::take(&mut self.buffer))));
            }
            self.buffer.extend_from_slice(&chunk[..length]);
        }
    }
}
