//! Turns a search outcome into the native Cursor card and the model's text.

use serde::Deserialize;

use crate::{
    cursor::{
        protocol::proto::agent::v1 as pb,
        tools::{codec, tool_call_result::ToolCompletion},
    },
    model::{ToolCall, ToolResult},
    Result,
};

#[derive(Debug, Deserialize)]
pub(super) struct Found {
    results: Vec<Location>,
}

#[derive(Debug, Deserialize)]
struct Location {
    path: String,
    start_line: i32,
    end_line: i32,
    score: f32,
    content: String,
}

pub(super) fn parse(stdout: &str) -> std::result::Result<Found, String> {
    serde_json::from_str(stdout.trim()).map_err(|error| {
        let head = stdout.trim().chars().take(200).collect::<String>();
        format!("SemanticSearch returned something other than results ({error}): {head}")
    })
}

pub(super) fn found(call: &ToolCall, started_at_ms: u64, found: Found) -> Result<ToolCompletion> {
    let text = text(&found);
    let code_results = found.results.into_iter().map(code_result).collect();
    complete(
        call,
        started_at_ms,
        text.clone(),
        pb::sem_search_tool_result::Result::Success(pb::SemSearchToolSuccess {
            results: text,
            code_results,
        }),
    )
}

pub(crate) fn failed(
    call: &ToolCall,
    started_at_ms: u64,
    message: String,
) -> Result<ToolCompletion> {
    complete(
        call,
        started_at_ms,
        message.clone(),
        pb::sem_search_tool_result::Result::Error(pb::SemSearchToolError {
            error_message: message,
        }),
    )
}

fn complete(
    call: &ToolCall,
    started_at_ms: u64,
    content: String,
    result: pb::sem_search_tool_result::Result,
) -> Result<ToolCompletion> {
    let is_error = matches!(result, pb::sem_search_tool_result::Result::Error(_));
    let mut tool = codec::render_tool_call(call, true)?.tool;
    if let Some(pb::tool_call::Tool::SemSearchToolCall(card)) = tool.as_mut() {
        card.result = Some(pb::SemSearchToolResult {
            result: Some(result),
        });
    }
    let tool = tool.ok_or_else(|| {
        crate::Error::Protocol(format!("tool {} has no Cursor representation", call.name))
    })?;
    Ok(ToolCompletion::new(
        call,
        started_at_ms,
        ToolResult {
            call_id: call.call_id.clone(),
            content,
            is_error,
            parts: Vec::new(),
            image: None,
        },
        tool,
    ))
}

fn text(found: &Found) -> String {
    if found.results.is_empty() {
        return "No code locations matched.".into();
    }
    let mut text = String::new();
    for result in &found.results {
        text.push_str(&format!(
            "{}:{}-{}\n```\n{}\n```\n\n",
            result.path,
            result.start_line,
            result.end_line,
            result.content.trim_end()
        ));
    }
    text.trim_end().into()
}

fn code_result(location: Location) -> pb::CodeResult {
    let position = |line| Some(pb::CursorPosition { line, column: 1 });
    pb::CodeResult {
        code_block: Some(pb::CodeBlock {
            relative_workspace_path: location.path,
            range: Some(pb::CursorRange {
                start_position: position(location.start_line),
                end_position: position(location.end_line),
            }),
            contents: location.content,
            ..Default::default()
        }),
        score: location.score,
    }
}
