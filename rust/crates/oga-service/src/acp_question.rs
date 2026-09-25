//! Permission requests outside the task's scope, put to a person one at a time
//! while the worker stays parked on them.

use std::{future::Future, sync::Mutex, time::Duration};

use tokio::{
    sync::{Notify, oneshot},
    time::Instant,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Answer {
    Allow,
    Refuse,
    /// Refuse the step, and hand the worker this instruction instead.
    Instead(String),
}

impl Answer {
    /// Anything that isn't a yes or a no is an instruction.
    pub(crate) fn read(reply: &str) -> Self {
        let reply = reply.trim();
        match reply
            .trim_end_matches(['.', '!'])
            .to_ascii_lowercase()
            .as_str()
        {
            "allow" | "allow it" | "yes" | "y" | "ok" | "okay" | "go ahead" => Self::Allow,
            "refuse" | "refuse it" | "no" | "n" | "don't" | "deny" => Self::Refuse,
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
    /// Tokio's mutex is fair, so queued questions open oldest first.
    turn: tokio::sync::Mutex<()>,
    slot: Mutex<Slot>,
    pub(crate) clock: TurnClock,
}

impl Questions {
    /// `None` when the run stopped first or `open` could not put the question
    /// to anyone.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_yes_or_no_answers_the_question_rather_than_instructing() {
        assert_eq!(Answer::read("Yes"), Answer::Allow);
        assert_eq!(Answer::read("allow."), Answer::Allow);
        assert_eq!(Answer::read("no"), Answer::Refuse);
        assert_eq!(
            Answer::read("keep the backup inside the repo"),
            Answer::Instead("keep the backup inside the repo".into())
        );
    }
}
