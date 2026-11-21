//! Covers the injected MCP routing and desktop command approval boundary.

use std::collections::BTreeMap;

use serde_json::json;

use super::*;
use crate::cursor::tools::runtime::CursorToolRuntime;

fn call(tool: &str, arguments: serde_json::Value) -> ToolCall {
    let arguments = json!({"server":"cursor-devtools", "toolName":tool, "arguments":arguments});
    ToolCall {
        index: 0,
        call_id: "local-tool-1".into(),
        model_call_id: "model-local-tool-1".into(),
        name: "CallMcpTool".into(),
        arguments_text: arguments.to_string(),
        arguments,
        argument_error: None,
    }
}

#[tokio::test]
async fn local_run_displays_the_exact_command_before_any_execution() {
    let runtime = CursorToolRuntime::default();
    let cmd = "printf cursor-devtools-approval";
    let (results, _) = crate::cursor::tools::tool_call_result::tool_result_channel();
    let response = start(
        &runtime,
        &results,
        &call("local_run", json!({"command":cmd})),
        0,
        &BTreeMap::new(),
        &ExecContext::default(),
    )
    .await
    .unwrap();
    assert!(response.completion.is_none());
    let query = response.messages[0].message.as_ref().unwrap();
    let pb::agent_server_message::Message::InteractionQuery(query) = query else {
        panic!("local_run must prompt in Cursor before running")
    };
    let Some(pb::interaction_query::Query::AskQuestionInteractionQuery(ask)) = &query.query else {
        panic!("expected an explicit approval question")
    };
    let args = ask.args.as_ref().unwrap();
    assert!(args.questions[0].prompt.contains(cmd));
    let pending = runtime.take_interaction(query.id).await.unwrap();
    let outcome = resume_interaction(
        &results,
        &WebSearch::built_in(),
        &WebFetch::built_in(),
        pending,
        &pb::InteractionResponse {
            id: query.id,
            result: None,
        },
    )
    .await
    .unwrap();
    let InteractionContinuation::Completed(result) = outcome else {
        panic!("missing approval must complete as a rejection")
    };
    assert!(result.result().is_error);
}

#[tokio::test]
async fn approved_local_run_returns_desktop_output_in_an_mcp_card() {
    let runtime = CursorToolRuntime::default();
    let (results, mut receiver) = crate::cursor::tools::tool_call_result::tool_result_channel();
    let command = if cfg!(windows) {
        "Write-Output cursor-devtools-local"
    } else {
        "printf cursor-devtools-local"
    };
    let response = start(
        &runtime,
        &results,
        &call("local_run", json!({"command": command})),
        0,
        &BTreeMap::new(),
        &ExecContext::default(),
    )
    .await
    .unwrap();
    let Some(pb::agent_server_message::Message::InteractionQuery(query)) =
        &response.messages[0].message
    else {
        panic!("approval missing")
    };
    let pending = runtime.take_interaction(query.id).await.unwrap();
    let outcome = resume_interaction(
        &results,
        &WebSearch::built_in(),
        &WebFetch::built_in(),
        pending,
        &pb::InteractionResponse {
            id: query.id,
            result: Some(
                pb::interaction_response::Result::AskQuestionInteractionResponse(
                    pb::AskQuestionInteractionResponse {
                        result: Some(pb::AskQuestionResult {
                            result: Some(pb::ask_question_result::Result::Success(
                                pb::AskQuestionSuccess {
                                    answers: vec![pb::ask_question_success::Answer {
                                        question_id: "byok-desktop-command".into(),
                                        selected_option_ids: vec!["run-once".into()],
                                        freeform_text: String::new(),
                                    }],
                                },
                            )),
                        }),
                    },
                ),
            ),
        },
    )
    .await
    .unwrap();
    assert!(matches!(outcome, InteractionContinuation::Pending));
    let completion = tokio::time::timeout(std::time::Duration::from_secs(5), receiver.recv())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(completion
        .result()
        .content
        .contains("cursor-devtools-local"));
    assert!(matches!(
        completion.tool_call().tool,
        Some(pb::tool_call::Tool::McpToolCall(_))
    ));
}
