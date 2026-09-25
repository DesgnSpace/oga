//! A step the task's scope does not cover, put to a person while the worker
//! stays parked on its permission request.
//!
//! One question is open at a time; the rest wait in the order the worker
//! asked them. The turn's clock stops while a question is open, so a person
//! may take as long as they like to answer.

use std::{future::Future, sync::Mutex, time::Duration};

use tokio::{
    sync::{Notify, oneshot},
    time::Instant,
};

/// What the person decided about the step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Answer {
    Allow,
    Refuse,
    /// Refuse the step, and hand the worker this instruction instead.
    Instead(String),
}

impl Answer {
    /// `allow` and `refuse` are the two buttons; anything else is an
    /// instruction.
    pub(crate) fn read(reply: &str) -> Self {
        let reply = reply.trim();
        match reply
            .trim_end_matches(['.', '!'])
            .to_ascii_lowercase()
            .as_str()
        {
            "allow" | "allow it" => Self::Allow,
            "refuse" | "refuse it" => Self::Refuse,
            _ => Self::Instead(reply.to_owned()),
        }
    }

    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Refuse => "refuse",
            Self::Instead(_) => "instead",
        }
    }
}

#[derive(Default)]
struct Slot {
    closed: bool,
    waiting: Option<oneshot::Sender<Answer>>,
}

#[derive(Default)]
pub(crate) struct Questions {
    /// Held by the question that is open; the rest queue on it in order.
    turn: tokio::sync::Mutex<()>,
    slot: Mutex<Slot>,
    pub(crate) clock: TurnClock,
}

impl Questions {
    /// Waits for this question's turn, opens it with `open`, and waits for
    /// the person's answer. `None` when the run stopped first or `open` could
    /// not put the question to anyone.
    pub(crate) async fn ask<Open>(&self, open: Open) -> Option<Answer>
    where
        Open: Future<Output = bool>,
    {
        let _turn = self.turn.lock().await;
        let (sender, answer) = oneshot::channel();
        {
            let mut slot = self.slot();
            if slot.closed {
                return None;
            }
            slot.waiting = Some(sender);
        }
        if !open.await {
            self.slot().waiting = None;
            return None;
        }
        self.clock.pause();
        let answer = answer.await.ok();
        self.clock.resume();
        answer
    }

    /// The open question's answer channel, taken so only one reply decides it.
    pub(crate) fn take(&self) -> Option<oneshot::Sender<Answer>> {
        self.slot().waiting.take()
    }

    /// Withdraws the open question and every one still queued.
    pub(crate) fn close(&self) {
        let mut slot = self.slot();
        slot.closed = true;
        slot.waiting = None;
    }

    fn slot(&self) -> std::sync::MutexGuard<'_, Slot> {
        self.slot
            .lock()
            .expect("question slot lock is not poisoned")
    }
}

/// The turn's deadline, which does not run while a person is being asked.
#[derive(Default)]
pub(crate) struct TurnClock {
    state: Mutex<ClockState>,
    resumed: Notify,
}

#[derive(Default)]
struct ClockState {
    deadline: Option<Instant>,
    paused_since: Option<Instant>,
}

impl TurnClock {
    pub(crate) fn start(&self, bound: Duration) {
        self.state().deadline = Some(Instant::now() + bound);
    }

    fn pause(&self) {
        self.state().paused_since.get_or_insert_with(Instant::now);
    }

    fn resume(&self) {
        {
            let mut state = self.state();
            if let Some(since) = state.paused_since.take()
                && let Some(deadline) = &mut state.deadline
            {
                *deadline += since.elapsed();
            }
        }
        self.resumed.notify_waiters();
    }

    /// Resolves once the turn has run for its whole bound, not counting the
    /// time spent waiting on a person.
    pub(crate) async fn expired(&self) {
        loop {
            // Taken before the state is read, so a resume in between still wakes it.
            let resumed = self.resumed.notified();
            let deadline = {
                let state = self.state();
                state.deadline.filter(|_| state.paused_since.is_none())
            };
            match deadline {
                None => resumed.await,
                Some(deadline) if Instant::now() >= deadline => return,
                Some(deadline) => tokio::select! {
                    () = tokio::time::sleep_until(deadline) => {}
                    () = resumed => {}
                },
            }
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, ClockState> {
        self.state.lock().expect("turn clock lock is not poisoned")
    }
}

/// The question in the person's words: the step, what it reaches, and how
/// to answer.
pub(crate) fn permission_question(
    command: Option<&str>,
    writes: bool,
    outside: &[String],
) -> String {
    let paths = outside.join(", ");
    let step = match command {
        Some(command) => {
            format!("run `{command}`, which reaches {paths}, outside what this task may use")
        }
        None if writes => format!("change {paths}, which this task isn't allowed to change"),
        None => format!("read {paths}, which this task isn't allowed to read"),
    };
    format!(
        "The worker wants to {step}. Allow it? Reply allow or refuse, or say what it should do instead."
    )
}
