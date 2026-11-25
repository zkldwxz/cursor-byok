//! Gives a mode to user messages that Cursor delivers without one.
use std::{collections::VecDeque, sync::Arc};

use parking_lot::Mutex;

use crate::cursor::protocol::proto::agent::v1::{
    self as pb, agent_client_message::Message, conversation_action::Action,
};

/// A choice outlives its message only when Cursor never delivers it.
const MAX_PENDING: usize = 256;

/// Cursor removes the mode from messages its remote controllers send but keeps
/// their id, so the mode a controller asked for waits here under that id.
#[derive(Clone, Default)]
pub struct MessageModes {
    chosen: Arc<Mutex<VecDeque<(String, pb::AgentMode)>>>,
}

impl MessageModes {
    pub fn choose(&self, message_id: String, mode: pb::AgentMode) {
        let mut chosen = self.chosen.lock();
        if chosen.len() == MAX_PENDING {
            chosen.pop_front();
        }
        chosen.push_back((message_id, mode));
    }

    /// A message with no mode of its own continues in its conversation's mode.
    pub fn fill(&self, message: &mut pb::AgentClientMessage) {
        let (action, conversation) = match &mut message.message {
            Some(Message::RunRequest(request)) if request.subagent_type_name.is_none() => {
                let mode = request
                    .conversation_state
                    .as_ref()
                    .map(pb::ConversationStateStructure::mode);
                (request.action.as_mut(), mode)
            }
            Some(Message::ConversationAction(action)) => (Some(action), None),
            _ => return,
        };
        let Some(Action::UserMessageAction(action)) = action.and_then(|it| it.action.as_mut())
        else {
            return;
        };
        let Some(user) = action.user_message.as_mut() else {
            return;
        };
        if user.mode() != pb::AgentMode::Unspecified {
            return;
        }
        let mode = self
            .chosen(&user.message_id)
            .or(conversation.filter(|mode| *mode != pb::AgentMode::Unspecified))
            .unwrap_or(pb::AgentMode::Agent);
        user.set_mode(mode);
    }

    fn chosen(&self, message_id: &str) -> Option<pb::AgentMode> {
        self.chosen
            .lock()
            .iter()
            .find(|(id, _)| id == message_id)
            .map(|(_, mode)| *mode)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(
        message_id: &str,
        mode: pb::AgentMode,
        conversation: Option<pb::AgentMode>,
    ) -> pb::AgentClientMessage {
        let action = pb::ConversationAction {
            action: Some(Action::UserMessageAction(pb::UserMessageAction {
                user_message: Some(pb::UserMessage {
                    message_id: message_id.into(),
                    mode: mode as i32,
                    ..Default::default()
                }),
                ..Default::default()
            })),
            ..Default::default()
        };
        pb::AgentClientMessage {
            message: Some(Message::RunRequest(pb::AgentRunRequest {
                action: Some(action),
                conversation_state: conversation.map(|mode| pb::ConversationStateStructure {
                    mode: Some(mode as i32),
                    ..Default::default()
                }),
                ..Default::default()
            })),
        }
    }

    fn mode(message: &pb::AgentClientMessage) -> pb::AgentMode {
        let Some(Message::RunRequest(request)) = &message.message else {
            panic!("not a run request");
        };
        let Some(Action::UserMessageAction(action)) =
            request.action.as_ref().and_then(|it| it.action.as_ref())
        else {
            panic!("not a user message");
        };
        action.user_message.as_ref().unwrap().mode()
    }

    #[test]
    fn a_remote_message_runs_in_the_mode_its_sender_chose() {
        let modes = MessageModes::default();
        modes.choose("a".into(), pb::AgentMode::Plan);
        let mut message = run("a", pb::AgentMode::Unspecified, Some(pb::AgentMode::Agent));
        modes.fill(&mut message);
        assert_eq!(mode(&message), pb::AgentMode::Plan);
    }

    #[test]
    fn a_message_without_a_choice_continues_the_conversations_mode() {
        let modes = MessageModes::default();
        let mut message = run("a", pb::AgentMode::Unspecified, Some(pb::AgentMode::Ask));
        modes.fill(&mut message);
        assert_eq!(mode(&message), pb::AgentMode::Ask);
        let mut message = run("a", pb::AgentMode::Unspecified, None);
        modes.fill(&mut message);
        assert_eq!(mode(&message), pb::AgentMode::Agent);
    }

    #[test]
    fn a_mode_cursor_sent_itself_is_never_replaced() {
        let modes = MessageModes::default();
        modes.choose("a".into(), pb::AgentMode::Agent);
        let mut message = run("a", pb::AgentMode::Plan, None);
        modes.fill(&mut message);
        assert_eq!(mode(&message), pb::AgentMode::Plan);
    }
}
