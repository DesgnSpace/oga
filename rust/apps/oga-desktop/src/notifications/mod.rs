//! Tells the user when a task stops, whether or not Oga is on screen.
//!
//! The shell already consumes every task's frames for the sidebar, so the same
//! stream decides what is worth interrupting for. Only the states a task
//! cannot leave on its own are worth a word: work that finished, work that
//! stopped short, and work that is waiting on an answer. Cancelling is the
//! user's own doing, so it passes in silence.

#[cfg(target_os = "macos")]
mod mac;

use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use oga_client::bridge::TaskFollower;
use oga_domain::{EventPointer, TaskState};
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tokio::sync::Notify;

use crate::lifecycle;

/// Carries the task a clicked notification asked to see.
pub const OPEN_TASK_EVENT: &str = "oga-open-task";

/// How long tasks that stop together are held, so a batch that lands at once
/// arrives as one notification per outcome instead of one per task.
const COALESCE_WINDOW: Duration = Duration::from_millis(1_500);
/// How many stops are remembered, so a stream that reconnects and replays
/// cannot say the same thing twice.
const REMEMBERED_STOPS: usize = 256;
/// The longest task name a notification carries.
const MAX_NAME: usize = 80;
/// How many task names a grouped notification lists before counting the rest.
const NAMED_IN_GROUP: usize = 3;

/// What the user is told about a task that stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Finished,
    NeedsAnswer,
    Stopped,
}

/// The order a burst is announced in: good news, then what is waiting on the
/// user, then what went wrong.
const OUTCOMES: [Outcome; 3] = [Outcome::Finished, Outcome::NeedsAnswer, Outcome::Stopped];

impl Outcome {
    fn of(state: TaskState) -> Option<Self> {
        match state {
            TaskState::Completed => Some(Self::Finished),
            TaskState::NeedsInput => Some(Self::NeedsAnswer),
            TaskState::Failed | TaskState::Blocked => Some(Self::Stopped),
            _ => None,
        }
    }

    fn line(self) -> &'static str {
        match self {
            Self::Finished => "Finished.",
            Self::NeedsAnswer => "Waiting on your answer.",
            Self::Stopped => "Stopped before it finished.",
        }
    }

    fn heading(self, count: usize) -> String {
        match self {
            Self::Finished => format!("{count} tasks finished"),
            Self::NeedsAnswer => format!("{count} tasks are waiting on you"),
            Self::Stopped => format!("{count} tasks stopped before they finished"),
        }
    }
}

/// A task that reached a state it cannot leave on its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stop {
    pub task_id: String,
    pub name: String,
    pub outcome: Outcome,
}

/// One notification, ready to hand to the platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub title: String,
    pub body: String,
    /// The task the notification opens, when it speaks for a single one.
    pub task_id: Option<String>,
}

/// Turns what stopped into what the user reads: one notification per outcome,
/// so twenty tasks landing together cost three banners rather than twenty.
pub fn messages(stops: &[Stop]) -> Vec<Message> {
    OUTCOMES
        .into_iter()
        .filter_map(|outcome| {
            let group: Vec<&Stop> = stops
                .iter()
                .filter(|stop| stop.outcome == outcome)
                .collect();
            match group.as_slice() {
                [] => None,
                [only] => Some(Message {
                    title: shorten(&only.name),
                    body: outcome.line().to_owned(),
                    task_id: Some(only.task_id.clone()),
                }),
                many => Some(Message {
                    title: outcome.heading(many.len()),
                    body: names(many),
                    task_id: None,
                }),
            }
        })
        .collect()
}

/// Reads as "One and Two", or "One, Two, Three and 2 more".
fn names(stops: &[&Stop]) -> String {
    let mut named: Vec<String> = stops
        .iter()
        .take(NAMED_IN_GROUP)
        .map(|stop| shorten(&stop.name))
        .collect();
    let last = match stops.len() - named.len() {
        0 => named.pop().unwrap_or_default(),
        1 => "1 more".to_owned(),
        unnamed => format!("{unnamed} more"),
    };
    if named.is_empty() {
        return last;
    }
    format!("{} and {last}", named.join(", "))
}

fn shorten(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.chars().count() <= MAX_NAME {
        return trimmed.to_owned();
    }
    let kept: String = trimmed.chars().take(MAX_NAME - 1).collect();
    format!("{}…", kept.trim_end())
}

#[derive(Default)]
struct Pending {
    stops: Vec<Stop>,
    seen: VecDeque<i64>,
}

/// Decides what the user hears about, and when.
pub struct Notifier<R: Runtime> {
    app: AppHandle<R>,
    follower: TaskFollower,
    enabled: Arc<AtomicBool>,
    pending: Arc<Mutex<Pending>>,
    stopped: Arc<Notify>,
}

impl<R: Runtime> Clone for Notifier<R> {
    fn clone(&self) -> Self {
        Self {
            app: self.app.clone(),
            follower: self.follower.clone(),
            enabled: self.enabled.clone(),
            pending: self.pending.clone(),
            stopped: self.stopped.clone(),
        }
    }
}

impl<R: Runtime> Notifier<R> {
    pub fn new(app: AppHandle<R>, follower: TaskFollower) -> Self {
        Self {
            app,
            follower,
            enabled: Arc::new(AtomicBool::new(true)),
            pending: Arc::new(Mutex::new(Pending::default())),
            stopped: Arc::new(Notify::new()),
        }
    }

    /// Starts the loop that holds a burst together and hands the platform the
    /// notifications it turned into, and readies the click route.
    pub fn start(&self) {
        #[cfg(target_os = "macos")]
        mac::route_clicks(self.app.clone());
        let notifier = self.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                notifier.stopped.notified().await;
                tokio::time::sleep(COALESCE_WINDOW).await;
                let stops = notifier.take();
                for message in messages(&stops) {
                    notifier.show(message);
                }
            }
        });
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
        if !enabled {
            self.take();
        }
    }

    /// Takes one frame from the broker stream.
    pub fn note(&self, pointer: &EventPointer) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        let Some(outcome) = Outcome::of(pointer.state) else {
            return;
        };
        if !is_stop(&pointer.event_type) || self.on_screen(&pointer.task_id) {
            return;
        }
        let mut pending = self.pending.lock().expect("pending notifications lock");
        if pending.seen.contains(&pointer.id) {
            return;
        }
        pending.seen.push_back(pointer.id);
        if pending.seen.len() > REMEMBERED_STOPS {
            pending.seen.pop_front();
        }
        pending.stops.push(Stop {
            task_id: pointer.task_id.clone(),
            name: task_name(pointer),
            outcome,
        });
        drop(pending);
        self.stopped.notify_one();
    }

    /// A task the user is already reading, in a window they are looking at,
    /// needs no banner: they are watching it land.
    fn on_screen(&self, task_id: &str) -> bool {
        if self.follower.watching().as_deref() != Some(task_id) {
            return false;
        }
        self.app
            .get_webview_window(lifecycle::MAIN_WINDOW_LABEL)
            .is_some_and(|window| {
                window.is_visible().unwrap_or(false) && window.is_focused().unwrap_or(false)
            })
    }

    fn take(&self) -> Vec<Stop> {
        std::mem::take(
            &mut self
                .pending
                .lock()
                .expect("pending notifications lock")
                .stops,
        )
    }

    fn show(&self, message: Message) {
        #[cfg(target_os = "macos")]
        mac::show(&self.app, message);
        #[cfg(not(target_os = "macos"))]
        {
            use tauri_plugin_notification::NotificationExt;
            let _ = self
                .app
                .notification()
                .builder()
                .title(message.title)
                .body(message.body)
                .show();
        }
    }
}

/// Brings the window forward on the task a notification was about.
fn open_task<R: Runtime>(app: &AppHandle<R>, task_id: &str) {
    lifecycle::show_main_window(app);
    let _ = app.emit(OPEN_TASK_EVENT, task_id);
}

/// What the user calls the task. A frame falls back to the task's id when the
/// work was never named, and an id says nothing to the person reading a banner.
fn task_name(pointer: &EventPointer) -> String {
    let title = pointer.title.trim();
    if title.is_empty() || title == pointer.task_id {
        return "Your task".to_owned();
    }
    title.to_owned()
}

/// The events a task writes when it stops. A task also carries its state on
/// every frame while it runs, so the state alone would fire on activity that
/// merely followed the stop.
fn is_stop(event_type: &str) -> bool {
    matches!(
        event_type,
        "completed" | "failed" | "needs_input" | "blocked"
    )
}

/// Turns notifications on or off for the rest of the session; the web view
/// holds the user's choice and pushes it down at startup.
#[tauri::command]
pub fn set_task_notifications(notifier: tauri::State<'_, Notifier<tauri::Wry>>, enabled: bool) {
    notifier.set_enabled(enabled);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stop(task_id: &str, name: &str, outcome: Outcome) -> Stop {
        Stop {
            task_id: task_id.to_owned(),
            name: name.to_owned(),
            outcome,
        }
    }

    #[test]
    fn a_task_that_stops_is_named_with_its_outcome() {
        let messages = messages(&[stop("t1", "Port the settings screen", Outcome::Finished)]);

        assert_eq!(
            messages,
            vec![Message {
                title: "Port the settings screen".to_owned(),
                body: "Finished.".to_owned(),
                task_id: Some("t1".to_owned()),
            }]
        );
    }

    #[test]
    fn failures_read_as_work_that_stopped() {
        let messages = messages(&[stop("t1", "Port the settings screen", Outcome::Stopped)]);

        assert_eq!(messages[0].body, "Stopped before it finished.");
    }

    #[test]
    fn tasks_landing_together_share_one_notification_per_outcome() {
        let messages = messages(&[
            stop("t1", "One", Outcome::Finished),
            stop("t2", "Two", Outcome::Stopped),
            stop("t3", "Three", Outcome::Finished),
        ]);

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].title, "2 tasks finished");
        assert_eq!(messages[0].body, "One and Three");
        assert_eq!(messages[0].task_id, None);
        assert_eq!(messages[1].title, "Two");
        assert_eq!(messages[1].body, "Stopped before it finished.");
    }

    #[test]
    fn a_long_burst_counts_the_tasks_it_does_not_name() {
        let stops: Vec<Stop> = ["One", "Two", "Three", "Four", "Five"]
            .into_iter()
            .enumerate()
            .map(|(index, name)| stop(&format!("t{index}"), name, Outcome::Finished))
            .collect();

        let messages = messages(&stops);

        assert_eq!(messages[0].title, "5 tasks finished");
        assert_eq!(messages[0].body, "One, Two, Three and 2 more");
    }

    #[test]
    fn a_waiting_task_asks_for_an_answer() {
        let messages = messages(&[stop("t1", "Ship the release", Outcome::NeedsAnswer)]);

        assert_eq!(messages[0].body, "Waiting on your answer.");
    }

    #[test]
    fn cancelling_a_task_says_nothing() {
        assert_eq!(Outcome::of(TaskState::Cancelled), None);
        assert_eq!(Outcome::of(TaskState::Running), None);
        assert_eq!(Outcome::of(TaskState::Completed), Some(Outcome::Finished));
    }

    #[test]
    fn only_the_events_a_stop_writes_are_worth_a_word() {
        assert!(is_stop("completed"));
        assert!(is_stop("failed"));
        assert!(!is_stop("agent.result"));
        assert!(!is_stop("state_changed"));
    }

    #[test]
    fn a_long_task_name_is_cut_to_fit_a_banner() {
        let name = "x".repeat(200);

        let shortened = shorten(&name);

        assert_eq!(shortened.chars().count(), MAX_NAME);
        assert!(shortened.ends_with('…'));
    }
}
