//! Builds one conversation, as the phone shows it, from Cursor's session events.
use std::collections::HashMap;

use serde::Serialize;
use serde_json::{json, Value};

use crate::cursor::protocol::proto::agent::v1::{
    self as pb, agent_host_session_event::Event, agent_host_turn_awaiting_input::Question,
    interaction_query::Query, interaction_update::Message as Update, tool_call::Tool,
};

use super::markdown::{self, Block};

const MAX_OUTPUT_CHARS: usize = 4_000;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub(super) struct Row {
    key: String,
    kind: &'static str,
    #[serde(skip_serializing_if = "String::is_empty")]
    text: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    blocks: Vec<Block>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool: Option<ToolView>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    todos: Vec<Todo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    seconds: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<&'static str>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub(super) struct ToolView {
    /// What the step does: run, edit, read, search or tool.
    pub kind: &'static str,
    pub title: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub detail: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub command: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub output: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub added: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub removed: Option<i32>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub(super) struct Todo {
    pub text: String,
    pub state: &'static str,
}

#[derive(Default)]
pub(super) struct Transcript {
    rows: Vec<Row>,
    tools: HashMap<String, usize>,
    pending: Vec<pb::AgentHostTurnAwaitingInput>,
    running: Option<String>,
    mode: Option<pb::AgentMode>,
    error: Option<String>,
    /// Text still being written into the last row.
    open: String,
    next_key: usize,
}

impl Transcript {
    /// Rows an attached session replays only as saved turns.
    pub fn restore(&mut self, turns: Vec<pb::AgentConversationTurn>) {
        for turn in turns {
            if let Some(user) = turn.user_message {
                self.human(&user);
            }
            for step in turn.steps {
                self.step(step);
            }
        }
        self.close();
    }

    pub fn apply(&mut self, event: Event) {
        match event {
            Event::InteractionUpdate(update) => {
                if let Some(update) = update.message {
                    self.update(update);
                }
            }
            Event::ConversationState(state) => self.state(&state),
            Event::TurnStarted(turn) => {
                self.error = None;
                self.running = Some(turn.turn_id);
            }
            Event::TurnAwaitingInput(asked) => {
                self.pending
                    .retain(|item| item.interaction_id != asked.interaction_id);
                self.pending.push(asked);
            }
            Event::InteractionResolved(resolved) => self
                .pending
                .retain(|item| item.interaction_id != resolved.interaction_id),
            Event::TurnSettled(settled) => {
                self.pending.retain(|item| item.turn_id != settled.turn_id);
                if self.running.as_deref() == Some(settled.turn_id.as_str()) {
                    self.running = None;
                }
                if settled.outcome() == pb::AgentHostTurnOutcome::Failed {
                    self.error = settled.detail;
                }
                self.close();
            }
            Event::SubagentStarted(_) => {}
        }
    }

    pub fn state(&mut self, state: &pb::ConversationStateStructure) {
        self.mode = Some(state.mode()).filter(|mode| *mode != pb::AgentMode::Unspecified);
    }

    /// A whole message from the person.
    pub fn said(&mut self, text: &str) {
        self.push("human").text = text.into();
    }

    pub fn thought(&mut self, text: &str) {
        if !text.trim().is_empty() {
            self.push("thinking").text = text.into();
        }
    }

    pub fn wrote(&mut self, text: &str) {
        let blocks = markdown::blocks(text);
        if !blocks.is_empty() {
            self.push("assistant").blocks = blocks;
        }
    }

    pub fn used(&mut self, tool: ToolView, todos: Vec<Todo>, status: Option<&'static str>) {
        let row = self.push("tool");
        (row.tool, row.todos, row.status) = (Some(tool), todos, status);
    }

    /// Whether a turn runs that only this server can name.
    pub fn run(&mut self, running: bool) {
        self.running = running.then(String::new);
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn running(&self) -> Option<&str> {
        self.running.as_deref()
    }

    pub fn mode(&self) -> Option<pb::AgentMode> {
        self.mode
    }

    pub fn pending(&self, id: &str) -> Option<&pb::AgentHostTurnAwaitingInput> {
        self.pending.iter().find(|item| item.interaction_id == id)
    }

    /// Everything but the rows.
    pub fn status(&self) -> Value {
        json!({
            "running": self.running.is_some(),
            "error": self.error,
            "asked": self.pending.iter().map(asked).collect::<Vec<_>>(),
        })
    }

    fn update(&mut self, update: Update) {
        match update {
            Update::UserMessageAppended(appended) => {
                if let Some(user) = appended.user_message {
                    self.human(&user);
                }
            }
            Update::TextDelta(delta) => {
                if self.last_kind() != Some("assistant") {
                    self.close();
                    self.push("assistant");
                }
                self.open.push_str(&delta.text);
                let blocks = markdown::blocks(&self.open);
                self.last().blocks = blocks;
            }
            Update::ThinkingDelta(delta) => {
                if self.last_kind() != Some("thinking") {
                    self.close();
                    self.push("thinking").status = Some("loading");
                }
                self.last().text.push_str(&delta.text);
            }
            Update::ThinkingCompleted(done) => {
                if self.last_kind() == Some("thinking") {
                    let row = self.last();
                    row.status = None;
                    row.seconds = Some(seconds(done.thinking_duration_ms.max(0) as u32));
                }
            }
            Update::ToolCallStarted(started) => {
                if let Some(call) = started.tool_call {
                    self.tool(started.call_id, call, false);
                }
            }
            Update::ToolCallCompleted(completed) => {
                if let Some(call) = completed.tool_call {
                    self.tool(completed.call_id, call, true);
                }
            }
            _ => {}
        }
    }

    fn step(&mut self, step: pb::ConversationStep) {
        use pb::conversation_step::Message;
        match step.message {
            Some(Message::AssistantMessage(message)) => {
                self.wrote(&message.text);
            }
            Some(Message::ThinkingMessage(thinking)) => {
                let row = self.push("thinking");
                row.text = thinking.text;
                row.seconds = Some(seconds(thinking.duration_ms));
            }
            Some(Message::ToolCall(call)) => {
                let id = call.tool_call_id.clone().unwrap_or_default();
                self.tool(id, call, true);
            }
            None => {}
        }
    }

    fn human(&mut self, user: &pb::UserMessage) {
        self.close();
        if !user.text.trim().is_empty() {
            self.push("human").text = user.text.clone();
        }
    }

    fn tool(&mut self, id: String, call: pb::ToolCall, done: bool) {
        let Some(tool) = call.tool else { return };
        let index = match self.tools.get(&id).filter(|_| !id.is_empty()) {
            Some(index) => *index,
            None => {
                self.close();
                self.push("tool");
                let index = self.rows.len() - 1;
                self.tools.insert(id, index);
                index
            }
        };
        let row = &mut self.rows[index];
        row.status = if !done {
            Some("loading")
        } else if failed(&tool) {
            Some("error")
        } else {
            None
        };
        if let Tool::UpdateTodosToolCall(call) = &tool {
            row.todos = todos(call);
        }
        row.tool = Some(view(&tool));
    }

    /// The last row is complete: what comes next starts its own.
    fn close(&mut self) {
        self.open.clear();
        if let Some(row) = self.rows.last_mut() {
            if row.kind == "thinking" {
                row.status = None;
            }
        }
    }

    fn push(&mut self, kind: &'static str) -> &mut Row {
        self.next_key += 1;
        self.rows.push(Row {
            key: self.next_key.to_string(),
            kind,
            text: String::new(),
            blocks: Vec::new(),
            tool: None,
            todos: Vec::new(),
            seconds: None,
            status: None,
        });
        self.last()
    }

    fn last(&mut self) -> &mut Row {
        self.rows.last_mut().expect("a row was just pushed")
    }

    /// The kind of the row still being written, if one is.
    fn last_kind(&self) -> Option<&'static str> {
        let row = self.rows.last()?;
        match row.kind {
            "assistant" if self.open.is_empty() => None,
            "thinking" if row.status.is_none() => None,
            kind => Some(kind),
        }
    }
}

fn seconds(milliseconds: u32) -> u32 {
    milliseconds.div_ceil(1_000)
}

pub(super) fn file_name(path: &str) -> String {
    path.rsplit(['/', '\\']).next().unwrap_or(path).into()
}

pub(super) fn tail(text: &str) -> String {
    let skip = text.chars().count().saturating_sub(MAX_OUTPUT_CHARS);
    text.chars().skip(skip).collect()
}

fn failed(tool: &Tool) -> bool {
    use pb::shell_result::Result as Shell;
    match tool {
        Tool::ShellToolCall(call) => call
            .result
            .as_ref()
            .and_then(|result| result.result.as_ref())
            .is_some_and(|result| !matches!(result, Shell::Success(_))),
        Tool::EditToolCall(call) => call
            .result
            .as_ref()
            .and_then(|result| result.result.as_ref())
            .is_some_and(|result| !matches!(result, pb::edit_result::Result::Success(_))),
        _ => false,
    }
}

fn todos(call: &pb::UpdateTodosToolCall) -> Vec<Todo> {
    let items = call.args.as_ref().map(|args| args.todos.as_slice());
    items
        .unwrap_or_default()
        .iter()
        .map(|todo| Todo {
            text: todo.content.clone(),
            state: match todo.status() {
                pb::TodoStatus::Completed => "completed",
                pb::TodoStatus::Cancelled => "cancelled",
                pb::TodoStatus::InProgress => "running",
                _ => "pending",
            },
        })
        .collect()
}

fn view(tool: &Tool) -> ToolView {
    let step = |kind, title: &str, detail: String| ToolView {
        kind,
        title: title.into(),
        detail,
        ..ToolView::default()
    };
    match tool {
        Tool::ShellToolCall(call) => {
            let args = call.args.clone().unwrap_or_default();
            ToolView {
                kind: "run",
                title: call
                    .description
                    .clone()
                    .filter(|text| !text.is_empty())
                    .unwrap_or_else(|| args.command.clone()),
                output: shell_output(call),
                command: args.command,
                ..ToolView::default()
            }
        }
        Tool::EditToolCall(call) => {
            let path = call.args.as_ref().map(|args| args.path.as_str());
            let mut view = step("edit", &file_name(path.unwrap_or_default()), String::new());
            if let Some(pb::edit_result::Result::Success(done)) = call
                .result
                .as_ref()
                .and_then(|result| result.result.as_ref())
            {
                view.added = done.lines_added;
                view.removed = done.lines_removed;
            }
            view
        }
        Tool::DeleteToolCall(call) => {
            let path = call.args.as_ref().map(|args| args.path.as_str());
            step("edit", "Delete", file_name(path.unwrap_or_default()))
        }
        Tool::ReadToolCall(call) => {
            let path = call.args.as_ref().map(|args| args.path.as_str());
            step("read", "Read", file_name(path.unwrap_or_default()))
        }
        Tool::LsToolCall(call) => {
            let path = call.args.as_ref().map(|args| args.path.as_str());
            step("search", "List", file_name(path.unwrap_or_default()))
        }
        Tool::GrepToolCall(call) => step(
            "search",
            "Grep",
            call.args
                .as_ref()
                .map(|args| args.pattern.clone())
                .unwrap_or_default(),
        ),
        Tool::GlobToolCall(call) => step(
            "search",
            "Find",
            call.args
                .as_ref()
                .map(|args| args.glob_pattern.clone())
                .unwrap_or_default(),
        ),
        Tool::SemSearchToolCall(call) => step(
            "search",
            "Search",
            call.args
                .as_ref()
                .map(|args| args.query.clone())
                .unwrap_or_default(),
        ),
        Tool::WebSearchToolCall(call) => step(
            "search",
            "Search the web",
            call.args
                .as_ref()
                .map(|args| args.search_term.clone())
                .unwrap_or_default(),
        ),
        Tool::WebFetchToolCall(call) => step(
            "tool",
            "Fetch",
            call.args
                .as_ref()
                .map(|args| args.url.clone())
                .unwrap_or_default(),
        ),
        Tool::McpToolCall(call) => {
            let args = call.args.clone().unwrap_or_default();
            step("tool", &args.tool_name, args.provider_identifier)
        }
        Tool::TaskToolCall(call) => step(
            "tool",
            "Subagent",
            call.args
                .as_ref()
                .map(|args| args.description.clone())
                .unwrap_or_default(),
        ),
        Tool::CreatePlanToolCall(call) => {
            let args = call.args.clone().unwrap_or_default();
            ToolView {
                kind: "tool",
                title: "Plan".into(),
                detail: args.name,
                output: args.plan,
                ..ToolView::default()
            }
        }
        Tool::UpdateTodosToolCall(_) => step("tool", "Todos", String::new()),
        Tool::AskQuestionToolCall(_) => step("tool", "Question", String::new()),
        Tool::SwitchModeToolCall(_) => step("tool", "Switch mode", String::new()),
        _ => step("tool", "Tool", String::new()),
    }
}

fn shell_output(call: &pb::ShellToolCall) -> String {
    use pb::shell_result::Result as Shell;
    let result = call
        .result
        .as_ref()
        .and_then(|result| result.result.as_ref());
    let (stdout, stderr) = match result {
        Some(Shell::Success(done)) => (&done.stdout, &done.stderr),
        Some(Shell::Failure(done)) => (&done.stdout, &done.stderr),
        _ => return String::new(),
    };
    tail([stdout.trim_end(), stderr.trim_end()].join("\n").trim())
}

/// What the agent stands still for. An unknown kind can be answered only on the computer.
fn asked(asked: &pb::AgentHostTurnAwaitingInput) -> Value {
    use pb::agent_host_approval_operation::Details;
    let view = |kind: &str, subject: &str, why: &str| json!({"id": asked.interaction_id, "kind": kind, "subject": subject, "why": why});
    match &asked.question {
        Some(Question::ExecApproval(request)) => {
            match request
                .operation
                .as_ref()
                .and_then(|it| it.details.as_ref())
            {
                Some(Details::Shell(it)) => view(
                    "command",
                    &it.command,
                    it.reason.as_deref().unwrap_or_default(),
                ),
                Some(Details::Write(it)) => view("write", &it.path, &it.reason),
                Some(Details::Read(it)) => {
                    view("read", &it.path, it.reason.as_deref().unwrap_or_default())
                }
                Some(Details::Delete(it)) => view("delete", &it.path, ""),
                Some(Details::Mcp(it)) => view(
                    "mcp",
                    &format!("{} {}", it.provider_identifier, it.tool_name),
                    it.reason.as_deref().unwrap_or_default(),
                ),
                None => view("unknown", "", ""),
            }
        }
        Some(Question::InteractionQuery(query)) => match &query.query {
            Some(Query::AskQuestionInteractionQuery(query)) => {
                let args = query.args.clone().unwrap_or_default();
                let mut view = view("questions", &args.title, "");
                view["list"] = args
                    .questions
                    .iter()
                    .map(|question| {
                        json!({
                            "id": question.id,
                            "text": question.prompt,
                            "multiple": question.allow_multiple,
                            "options": question.options.iter().map(|option| json!({"id": option.id, "label": option.label})).collect::<Vec<_>>(),
                        })
                    })
                    .collect();
                view
            }
            Some(Query::WebSearchRequestQuery(query)) => view(
                "web_search",
                query
                    .args
                    .as_ref()
                    .map(|args| args.search_term.as_str())
                    .unwrap_or_default(),
                "",
            ),
            Some(Query::WebFetchRequestQuery(query)) => view(
                "web_fetch",
                query
                    .args
                    .as_ref()
                    .map(|args| args.url.as_str())
                    .unwrap_or_default(),
                "",
            ),
            Some(Query::SwitchModeRequestQuery(query)) => {
                let args = query.args.clone().unwrap_or_default();
                view(
                    "switch_mode",
                    &args.target_mode_id,
                    args.explanation.as_deref().unwrap_or_default(),
                )
            }
            Some(Query::CreatePlanRequestQuery(query)) => {
                let args = query.args.clone().unwrap_or_default();
                view("plan", &args.name, &args.overview)
            }
            _ => view("unknown", "", ""),
        },
        _ => view("unknown", "", ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn update(message: Update) -> Event {
        Event::InteractionUpdate(pb::InteractionUpdate {
            message: Some(message),
        })
    }

    fn says(text: &str) -> Event {
        update(Update::TextDelta(pb::TextDeltaUpdate {
            text: text.into(),
            ..Default::default()
        }))
    }

    fn shell(command: &str, result: Option<pb::shell_result::Result>) -> pb::ToolCall {
        pb::ToolCall {
            tool: Some(Tool::ShellToolCall(pb::ShellToolCall {
                args: Some(pb::ShellArgs {
                    command: command.into(),
                    ..Default::default()
                }),
                result: result.map(|result| pb::ShellResult {
                    result: Some(result),
                    ..Default::default()
                }),
                ..Default::default()
            })),
            ..Default::default()
        }
    }

    fn runs(id: &str, call: pb::ToolCall) -> Event {
        update(Update::ToolCallStarted(pb::ToolCallStartedUpdate {
            call_id: id.into(),
            tool_call: Some(call),
            ..Default::default()
        }))
    }

    fn ran(id: &str, call: pb::ToolCall) -> Event {
        update(Update::ToolCallCompleted(pb::ToolCallCompletedUpdate {
            call_id: id.into(),
            tool_call: Some(call),
            ..Default::default()
        }))
    }

    fn shown(transcript: &Transcript) -> Value {
        serde_json::to_value(transcript.rows()).unwrap()
    }

    #[test]
    fn streamed_text_grows_one_row_until_something_else_happens() {
        let mut transcript = Transcript::default();
        for event in [
            says("Hel"),
            says("lo"),
            runs("t", shell("ls", None)),
            says("Done"),
        ] {
            transcript.apply(event);
        }
        let rows = shown(&transcript);
        assert_eq!(rows.as_array().unwrap().len(), 3);
        assert_eq!(rows[0]["blocks"][0]["runs"][0]["text"], "Hello");
        assert_eq!(rows[1]["status"], "loading");
        assert_eq!(rows[2]["blocks"][0]["runs"][0]["text"], "Done");
    }

    #[test]
    fn a_finished_command_keeps_its_row_and_shows_its_output_or_failure() {
        let mut transcript = Transcript::default();
        transcript.apply(runs("a", shell("ls", None)));
        transcript.apply(runs("b", shell("false", None)));
        transcript.apply(ran(
            "a",
            shell(
                "ls",
                Some(pb::shell_result::Result::Success(pb::ShellSuccess {
                    stdout: "a.txt\n".into(),
                    ..Default::default()
                })),
            ),
        ));
        transcript.apply(ran(
            "b",
            shell(
                "false",
                Some(pb::shell_result::Result::Failure(pb::ShellFailure {
                    stderr: "no".into(),
                    ..Default::default()
                })),
            ),
        ));
        let rows = shown(&transcript);
        assert_eq!(rows.as_array().unwrap().len(), 2);
        assert_eq!(rows[0]["tool"]["output"], "a.txt");
        assert_eq!(rows[0].get("status"), None);
        assert_eq!(
            (&rows[1]["status"], &rows[1]["tool"]["output"]),
            (&json!("error"), &json!("no"))
        );
    }

    #[test]
    fn saved_turns_read_like_live_ones() {
        let mut transcript = Transcript::default();
        transcript.restore(vec![pb::AgentConversationTurn {
            user_message: Some(pb::UserMessage {
                text: "hi".into(),
                ..Default::default()
            }),
            steps: vec![pb::ConversationStep {
                message: Some(pb::conversation_step::Message::AssistantMessage(
                    pb::AssistantMessage {
                        text: "hello".into(),
                    },
                )),
            }],
            ..Default::default()
        }]);
        transcript.apply(says("again"));
        let rows = shown(&transcript);
        assert_eq!(rows[0]["text"], "hi");
        assert_eq!(rows[1]["blocks"][0]["runs"][0]["text"], "hello");
        assert_eq!(rows[2]["blocks"][0]["runs"][0]["text"], "again");
    }

    #[test]
    fn a_question_stays_until_it_is_resolved_or_its_turn_ends() {
        let ask = |id: &str| {
            Event::TurnAwaitingInput(pb::AgentHostTurnAwaitingInput {
                turn_id: "turn".into(),
                interaction_id: id.into(),
                question: Some(Question::ExecApproval(pb::AgentHostApprovalRequest {
                    operation: Some(pb::AgentHostApprovalOperation {
                        details: Some(pb::agent_host_approval_operation::Details::Shell(
                            pb::AgentHostApprovalShellDetails {
                                command: "rm x".into(),
                                reason: Some("why".into()),
                                ..Default::default()
                            },
                        )),
                    }),
                    ..Default::default()
                })),
                ..Default::default()
            })
        };
        let mut transcript = Transcript::default();
        transcript.apply(Event::TurnStarted(pb::AgentHostTurnStarted {
            turn_id: "turn".into(),
        }));
        transcript.apply(ask("a"));
        transcript.apply(ask("b"));
        assert_eq!(
            transcript.status()["asked"][0],
            json!({"id": "a", "kind": "command", "subject": "rm x", "why": "why"})
        );
        transcript.apply(Event::InteractionResolved(
            pb::AgentHostInteractionResolved {
                interaction_id: "a".into(),
                ..Default::default()
            },
        ));
        assert!(transcript.pending("a").is_none());
        transcript.apply(Event::TurnSettled(pb::AgentHostTurnSettled {
            turn_id: "turn".into(),
            outcome: pb::AgentHostTurnOutcome::Failed as i32,
            detail: Some("broke".into()),
        }));
        assert_eq!(
            transcript.status(),
            json!({"running": false, "error": "broke", "asked": []})
        );
    }
}
