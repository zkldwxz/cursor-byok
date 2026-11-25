//! Tells which conversations wait for their model to be reached again.
use std::{collections::HashMap, sync::Arc};

use parking_lot::Mutex;

use crate::run::RunEvent;

#[derive(Clone, Default)]
pub struct UpstreamRetries {
    waiting: Arc<Mutex<HashMap<String, u32>>>,
}

impl UpstreamRetries {
    /// Which retry the conversation is on; `None` while its model answers.
    pub fn attempt(&self, conversation: &str) -> Option<u32> {
        self.waiting.lock().get(conversation).copied()
    }

    pub(crate) fn of(&self, conversation: &str) -> UpstreamRetry {
        UpstreamRetry {
            retries: self.clone(),
            conversation: conversation.into(),
        }
    }
}

/// One run's part; a run that ends, however it does, waits no more.
pub(crate) struct UpstreamRetry {
    retries: UpstreamRetries,
    conversation: String,
}

impl UpstreamRetry {
    pub fn hear(&self, event: &RunEvent) {
        match event {
            RunEvent::ModelAttemptFailed { attempt, .. } => {
                let mut waiting = self.retries.waiting.lock();
                waiting.insert(self.conversation.clone(), *attempt);
            }
            RunEvent::TextStart
            | RunEvent::ThinkingStart
            | RunEvent::ToolCallStart { .. }
            | RunEvent::CycleInterrupted => self.answered(),
            _ => {}
        }
    }

    fn answered(&self) {
        self.retries.waiting.lock().remove(&self.conversation);
    }
}

impl Drop for UpstreamRetry {
    fn drop(&mut self) {
        self.answered();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failed(attempt: u32) -> RunEvent {
        RunEvent::ModelAttemptFailed {
            attempt,
            message: "busy".into(),
        }
    }

    #[test]
    fn a_conversation_waits_from_a_failed_attempt_until_its_model_answers() {
        let retries = UpstreamRetries::default();
        let run = retries.of("one");
        run.hear(&failed(1));
        run.hear(&failed(2));
        assert_eq!(retries.attempt("one"), Some(2));
        assert_eq!(retries.attempt("other"), None);
        run.hear(&RunEvent::ThinkingStart);
        assert_eq!(retries.attempt("one"), None);
    }

    #[test]
    fn a_run_that_ends_while_waiting_leaves_nothing_behind() {
        let retries = UpstreamRetries::default();
        let run = retries.of("one");
        run.hear(&failed(8));
        drop(run);
        assert_eq!(retries.attempt("one"), None);
    }
}
