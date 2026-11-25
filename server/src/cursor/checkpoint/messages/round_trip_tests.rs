//! Proves the provider-visible history survives the Cursor history round trip.

use crate::model::{
    project_messages, project_messages_with_ends, CanonicalMessage, ContentPart, MessageContent,
    Origin, ProjectedContent, ProjectedMessage, Role, ToolCallContent, ToolResultContent,
};

use super::{decode, encode::wire_message};

/// Encodes the projected history the way a checkpoint publishes it, then
/// decodes the list back the way Cursor history hydration does — with fresh
/// `cursor-root:<blob>` identities, since the client returns our blobs under
/// its own root ordinal, not our internal message ids.
fn round_trip(messages: &[CanonicalMessage]) -> Vec<CanonicalMessage> {
    let encoded = project_messages_with_ends(messages)
        .unwrap()
        .into_iter()
        .map(|(message, _)| {
            serde_json::to_vec(&wire_message(&message, "claude", None).unwrap()).unwrap()
        })
        .collect::<Vec<_>>();
    encoded
        .iter()
        .enumerate()
        .map(|(ordinal, bytes)| decode(bytes, format!("cursor-root:{ordinal}:blob")).unwrap())
        .collect()
}

/// The provider wire formats carry no message ids, so identity differences
/// after hydration are invisible to the model; content and grouping are what
/// decide the cache prefix.
fn provider_shape(messages: &[CanonicalMessage]) -> Vec<(Role, ProjectedContent)> {
    let mut projected = project_messages(messages).unwrap();
    for message in &mut projected {
        message.message_id = String::new();
    }
    projected
        .into_iter()
        .map(|ProjectedMessage { role, content, .. }| (role, content))
        .collect()
}

fn call(index: usize, call_id: &str) -> ToolCallContent {
    ToolCallContent {
        index,
        call_id: call_id.into(),
        name: "Read".into(),
        arguments: serde_json::json!({"path": format!("/file-{index}")}),
    }
}

fn round_assistant(message_id: &str, round: &str, call: ToolCallContent) -> CanonicalMessage {
    CanonicalMessage {
        message_id: message_id.into(),
        role: Role::Assistant,
        origin: Origin::Assistant,
        content: MessageContent::Assistant {
            text: String::new(),
            thinking: String::new(),
            tool_round_id: Some(round.into()),
            replay_state: None,
            tool_calls: vec![call],
        },
        runtime_event_id: None,
    }
}

fn result(message_id: &str, call_id: &str) -> CanonicalMessage {
    CanonicalMessage {
        message_id: message_id.into(),
        role: Role::Tool,
        origin: Origin::Tool,
        content: MessageContent::ToolResult(ToolResultContent {
            call_id: call_id.into(),
            name: "Read".into(),
            content: "ok".into(),
            is_error: false,
            image: None,
            provider_parts: Vec::new(),
        }),
        runtime_event_id: None,
    }
}

fn user(message_id: &str, text: &str) -> CanonicalMessage {
    CanonicalMessage::text(message_id, Role::User, Origin::User, text)
}

/// What the run committed must project identically after Cursor hands the
/// history back. A multi-call round is committed as one assistant per call;
/// if the round trip splits or regroups them, the provider sees a different
/// history on the next call and the Anthropic prompt cache dies at that point.
#[test]
fn a_multi_call_tool_round_keeps_its_provider_shape_after_a_history_round_trip() {
    let committed = vec![
        user("user-1", "hello"),
        round_assistant("round:a1", "round-1", call(0, "call-1")),
        result("round:r1", "call-1"),
        round_assistant("round:a2", "round-1", call(1, "call-2")),
        result("round:r2", "call-2"),
        user("user-2", "go on"),
    ];
    let before = provider_shape(&committed);
    let after = provider_shape(&round_trip(&committed));

    assert_eq!(before, after, "the provider-visible shape must not change");
}

/// Single-call rounds already survive the trip; keep them green while fixing
/// the multi-call grouping.
#[test]
fn a_single_call_tool_round_keeps_its_provider_shape_after_a_history_round_trip() {
    let committed = vec![
        user("user-1", "hello"),
        round_assistant("round:a1", "round-1", call(0, "call-1")),
        result("round:r1", "call-1"),
        user("user-2", "go on"),
    ];
    let before = provider_shape(&committed);
    let after = provider_shape(&round_trip(&committed));

    assert_eq!(before, after);
}

/// A user message and a request-context message must round-trip untouched so
/// the appended prefix stays stable.
#[test]
fn user_and_request_context_messages_keep_their_provider_shape() {
    let committed = vec![
        CanonicalMessage {
            message_id: "request-context:digest".into(),
            role: Role::User,
            origin: Origin::Prompt,
            content: MessageContent::Parts {
                parts: vec![ContentPart::Text {
                    text: "<rules>current rules</rules>".into(),
                }],
            },
            runtime_event_id: None,
        },
        user("user-1", "hello"),
    ];
    let before = provider_shape(&committed);
    let after = provider_shape(&round_trip(&committed));

    assert_eq!(before, after);
}
