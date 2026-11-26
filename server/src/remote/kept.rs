//! Shows a conversation from the messages this server kept of it.
use std::collections::HashMap;

use serde_json::Value;

use crate::model::{
    CanonicalMessage, ContentPart, MessageContent, Role, ToolCallContent, ToolResultContent,
};

use super::transcript::{file_name, tail, Todo, ToolView, Transcript};

/// As for Cursor's own: older turns have no use on a phone.
const TURNS: usize = 40;

pub(super) fn transcript(messages: &[CanonicalMessage], running: bool) -> Transcript {
    let mut transcript = Transcript::default();
    let results = messages
        .iter()
        .filter_map(|message| match &message.content {
            MessageContent::ToolResult(result) => Some((result.call_id.as_str(), result)),
            _ => None,
        })
        .collect::<HashMap<_, _>>();
    for message in &messages[first(messages)..] {
        match &message.content {
            MessageContent::Parts { .. } => {
                if let Some(text) = asked(message) {
                    transcript.said(text);
                }
            }
            MessageContent::Assistant {
                text,
                thinking,
                tool_calls,
                ..
            } => {
                transcript.thought(thinking);
                transcript.wrote(text);
                for call in tool_calls {
                    let result = results.get(call.call_id.as_str()).copied();
                    let status = match result {
                        Some(result) if result.is_error => Some("error"),
                        None if running => Some("loading"),
                        _ => None,
                    };
                    transcript.used(view(call, result), todos(call), status);
                }
            }
            MessageContent::ToolResult(_) => {}
        }
    }
    transcript.run(running);
    transcript
}

/// Where the last `TURNS` begin.
fn first(messages: &[CanonicalMessage]) -> usize {
    let turns = messages.iter().enumerate().rev();
    turns
        .filter(|(_, message)| asked(message).is_some())
        .nth(TURNS - 1)
        .map_or(0, |(index, _)| index)
}

/// What the person wrote: everything else in a user message is for the model.
fn asked(message: &CanonicalMessage) -> Option<&str> {
    let (Role::User, MessageContent::Parts { parts }) = (&message.role, &message.content) else {
        return None;
    };
    parts.iter().rev().find_map(|part| {
        let ContentPart::Text { text } = part else {
            return None;
        };
        let (_, query) = text.rsplit_once("<user_query>")?;
        let query = query.split("</user_query>").next()?.trim();
        (!query.is_empty()).then_some(query)
    })
}

fn view(call: &ToolCallContent, result: Option<&ToolResultContent>) -> ToolView {
    let text = |name: &str| {
        let value = call.arguments.get(name).and_then(Value::as_str);
        value.unwrap_or_default().to_owned()
    };
    let step = |kind, title: &str, detail: String| ToolView {
        kind,
        title: title.into(),
        detail,
        ..ToolView::default()
    };
    match call.name.as_str() {
        "Shell" => {
            let description = text("description");
            let command = text("command");
            ToolView {
                kind: "run",
                title: if description.is_empty() {
                    command.clone()
                } else {
                    description
                },
                command,
                output: result.map_or_else(String::new, |it| tail(it.content.trim())),
                ..ToolView::default()
            }
        }
        "StrReplace" | "Write" => step("edit", &file_name(&text("path")), String::new()),
        "EditNotebook" => step("edit", &file_name(&text("target_notebook")), String::new()),
        "Delete" => step("edit", "Delete", file_name(&text("path"))),
        "Read" => step("read", "Read", file_name(&text("path"))),
        "Grep" => step("search", "Grep", text("pattern")),
        "Glob" => step("search", "Find", text("glob_pattern")),
        "SemanticSearch" => step("search", "Search", text("query")),
        "WebSearch" => step("search", "Search the web", text("search_term")),
        "WebFetch" => step("tool", "Fetch", text("url")),
        "CallMcpTool" => step("tool", &text("toolName"), text("server")),
        "Task" => step("tool", "Subagent", text("description")),
        "CreatePlan" => ToolView {
            output: text("plan"),
            ..step("tool", "Plan", text("name"))
        },
        "TodoWrite" => step("tool", "Todos", String::new()),
        "AskQuestion" => step("tool", "Question", String::new()),
        "SwitchMode" => step("tool", "Switch mode", String::new()),
        other => step("tool", other, String::new()),
    }
}

fn todos(call: &ToolCallContent) -> Vec<Todo> {
    if call.name != "TodoWrite" {
        return Vec::new();
    }
    let items = call.arguments.get("todos").and_then(Value::as_array);
    items
        .into_iter()
        .flatten()
        .map(|todo| Todo {
            text: todo["content"].as_str().unwrap_or_default().into(),
            state: match todo["status"].as_str() {
                Some("completed") => "completed",
                Some("cancelled") => "cancelled",
                Some("in_progress") => "running",
                _ => "pending",
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::model::Origin;

    fn user(id: &str, text: &str) -> CanonicalMessage {
        CanonicalMessage::text(id, Role::User, Origin::User, text)
    }

    fn assistant(id: &str, text: &str, calls: Vec<(&str, &str, Value)>) -> CanonicalMessage {
        let mut message = CanonicalMessage::text(id, Role::Assistant, Origin::Assistant, "");
        message.content = MessageContent::Assistant {
            text: text.into(),
            thinking: String::new(),
            tool_round_id: None,
            replay_state: None,
            tool_calls: calls
                .into_iter()
                .enumerate()
                .map(|(index, (id, name, arguments))| ToolCallContent {
                    index,
                    call_id: id.into(),
                    name: name.into(),
                    arguments,
                })
                .collect(),
        };
        message
    }

    fn result(id: &str, content: &str, is_error: bool) -> CanonicalMessage {
        let mut message = CanonicalMessage::text(id, Role::Tool, Origin::Tool, "");
        message.content = MessageContent::ToolResult(ToolResultContent {
            call_id: id.into(),
            name: "Shell".into(),
            content: content.into(),
            is_error,
            image: None,
            provider_parts: Vec::new(),
        });
        message
    }

    fn shown(messages: &[CanonicalMessage], running: bool) -> Value {
        serde_json::to_value(transcript(messages, running).rows()).unwrap()
    }

    #[test]
    fn a_person_sees_their_own_words_and_none_of_what_was_added_for_the_model() {
        let rows = shown(
            &[
                user("context", "<rules>many</rules>"),
                user(
                    "ask",
                    "<timestamp>now</timestamp>\n<user_query>\nfix it\n</user_query>",
                ),
                assistant("answer", "Done", vec![]),
            ],
            false,
        );
        assert_eq!(rows.as_array().unwrap().len(), 2);
        assert_eq!(rows[0]["text"], "fix it");
        assert_eq!(rows[1]["blocks"][0]["runs"][0]["text"], "Done");
    }

    #[test]
    fn a_command_shows_its_outcome_and_one_without_is_running_only_while_the_run_is() {
        let messages = [
            assistant(
                "a",
                "",
                vec![
                    ("ok", "Shell", json!({"command": "ls"})),
                    (
                        "bad",
                        "Shell",
                        json!({"command": "rm", "description": "Tidy"}),
                    ),
                    ("open", "Read", json!({"path": "/srv/app/main.rs"})),
                ],
            ),
            result("ok", "a.txt\n", false),
            result("bad", "denied", true),
        ];
        let rows = shown(&messages, true);
        assert_eq!(rows[0]["tool"]["output"], "a.txt");
        assert_eq!(rows[1]["tool"]["title"], "Tidy");
        assert_eq!(rows[1]["status"], "error");
        assert_eq!(rows[2]["tool"]["detail"], "main.rs");
        assert_eq!(rows[2]["status"], "loading");
        assert_eq!(shown(&messages, false)[2]["status"], Value::Null);
    }

    #[test]
    fn only_the_latest_turns_are_shown() {
        let messages = (0..TURNS + 3)
            .map(|turn| {
                user(
                    &turn.to_string(),
                    &format!("<user_query>{turn}</user_query>"),
                )
            })
            .collect::<Vec<_>>();
        let rows = shown(&messages, false);
        assert_eq!(rows.as_array().unwrap().len(), TURNS);
        assert_eq!(rows[0]["text"], "3");
    }
}
