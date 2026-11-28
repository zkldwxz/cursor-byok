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

fn approval_query(messages: &[pb::AgentServerMessage]) -> (u32, &pb::AskQuestionInteractionQuery) {
    messages
        .iter()
        .find_map(|message| match message.message.as_ref()? {
            pb::agent_server_message::Message::InteractionQuery(query) => {
                match query.query.as_ref()? {
                    pb::interaction_query::Query::AskQuestionInteractionQuery(ask) => {
                        Some((query.id, ask))
                    }
                    _ => None,
                }
            }
            _ => None,
        })
        .expect("local_run must ask for approval in Cursor before running")
}

fn answer(id: u32, option: &str) -> pb::InteractionResponse {
    pb::InteractionResponse {
        id,
        result: Some(
            pb::interaction_response::Result::AskQuestionInteractionResponse(
                pb::AskQuestionInteractionResponse {
                    result: Some(pb::AskQuestionResult {
                        result: Some(pb::ask_question_result::Result::Success(
                            pb::AskQuestionSuccess {
                                answers: vec![pb::ask_question_success::Answer {
                                    question_id: "byok-desktop-command".into(),
                                    selected_option_ids: vec![option.into()],
                                    freeform_text: String::new(),
                                }],
                            },
                        )),
                    }),
                },
            ),
        ),
    }
}

fn started_ids(messages: &[pb::AgentServerMessage]) -> Vec<&str> {
    updates(messages)
        .filter_map(|update| match update {
            pb::interaction_update::Message::ToolCallStarted(started) => {
                Some(started.call_id.as_str())
            }
            _ => None,
        })
        .collect()
}

fn completed_ids(messages: &[pb::AgentServerMessage]) -> Vec<&str> {
    updates(messages)
        .filter_map(|update| match update {
            pb::interaction_update::Message::ToolCallCompleted(completed) => {
                Some(completed.call_id.as_str())
            }
            _ => None,
        })
        .collect()
}

fn updates(
    messages: &[pb::AgentServerMessage],
) -> impl Iterator<Item = &pb::interaction_update::Message> {
    messages
        .iter()
        .filter_map(|message| match message.message.as_ref()? {
            pb::agent_server_message::Message::InteractionUpdate(update) => update.message.as_ref(),
            _ => None,
        })
}

async fn start_call(
    runtime: &CursorToolRuntime,
    results: &ToolResultSender,
    call: &ToolCall,
) -> ToolStart {
    start_with_store(runtime, results, None, call).await
}

async fn start_with_store(
    runtime: &CursorToolRuntime,
    results: &ToolResultSender,
    store: Option<&crate::store::Store>,
    call: &ToolCall,
) -> ToolStart {
    start(
        runtime,
        results,
        &crate::cursor::tools::semantic_search::SemanticPackage::built_in(),
        store,
        call,
        0,
        &BTreeMap::new(),
        &ExecContext::default(),
    )
    .await
    .unwrap()
}

async fn respond(
    runtime: &CursorToolRuntime,
    results: &ToolResultSender,
    response: &pb::InteractionResponse,
) -> ToolStart {
    respond_with_store(runtime, results, None, response).await
}

async fn respond_with_store(
    runtime: &CursorToolRuntime,
    results: &ToolResultSender,
    store: Option<&crate::store::Store>,
    response: &pb::InteractionResponse,
) -> ToolStart {
    let pending = runtime.take_interaction(response.id).await.unwrap();
    resume_interaction(
        results,
        &WebSearch::built_in(),
        &WebFetch::built_in(),
        None,
        store,
        pending,
        response,
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn local_run_displays_the_exact_command_before_any_execution() {
    let runtime = CursorToolRuntime::default();
    let cmd = "printf cursor-devtools-approval";
    let (results, _) = crate::cursor::tools::tool_call_result::tool_result_channel();
    let started = start_call(
        &runtime,
        &results,
        &call("local_run", json!({"command":cmd})),
    )
    .await;
    assert!(started.completion.is_none());
    let (id, ask) = approval_query(&started.messages);
    assert!(ask.args.as_ref().unwrap().questions[0].prompt.contains(cmd));
    let outcome = respond(
        &runtime,
        &results,
        &pb::InteractionResponse { id, result: None },
    )
    .await;
    let result = outcome
        .completion
        .expect("missing approval must complete as a rejection");
    assert!(result.result().is_error);
}

// Cursor looks up the question card by tool_call_id and reuses any card it
// already has; reusing the MCP card's id hides the question and leaves the
// conversation waiting with its spinner stopped.
#[tokio::test]
async fn local_run_approval_opens_its_own_question_card() {
    let runtime = CursorToolRuntime::default();
    let (results, _) = crate::cursor::tools::tool_call_result::tool_result_channel();
    let mcp_call = call("local_run", json!({"command":"printf approval-card"}));
    let started = start_call(&runtime, &results, &mcp_call).await;
    let (_, ask) = approval_query(&started.messages);
    assert!(!ask.tool_call_id.is_empty());
    assert_ne!(ask.tool_call_id, mcp_call.call_id);
    assert_eq!(started_ids(&started.messages), [ask.tool_call_id.as_str()]);
}

// Without this echo Cursor treats the answer as lost after 30 seconds and
// interrupts the run to redeliver it, while the command may still be running.
#[tokio::test]
async fn answering_the_approval_settles_the_question_card_without_waiting_for_the_command() {
    for option in ["run-once", "deny"] {
        let runtime = CursorToolRuntime::default();
        let (results, _receiver) = crate::cursor::tools::tool_call_result::tool_result_channel();
        let mcp_call = call("local_run", json!({"command":"printf settled"}));
        let started = start_call(&runtime, &results, &mcp_call).await;
        let (id, ask) = approval_query(&started.messages);
        let outcome = respond(&runtime, &results, &answer(id, option)).await;
        assert_eq!(
            completed_ids(&outcome.messages),
            [ask.tool_call_id.as_str()],
            "{option}"
        );
        assert_eq!(outcome.completion.is_some(), option == "deny", "{option}");
    }
}

#[tokio::test]
async fn a_directory_removed_before_approval_fails_the_call_and_still_settles_the_question() {
    let runtime = CursorToolRuntime::default();
    let (results, _receiver) = crate::cursor::tools::tool_call_result::tool_result_channel();
    let directory = tempfile::tempdir().unwrap();
    let mcp_call = call(
        "local_run",
        json!({"command":"printf gone", "cwd":directory.path()}),
    );
    let started = start_call(&runtime, &results, &mcp_call).await;
    let (id, ask) = approval_query(&started.messages);
    directory.close().unwrap();
    let outcome = respond(&runtime, &results, &answer(id, "run-once")).await;
    assert_eq!(
        completed_ids(&outcome.messages),
        [ask.tool_call_id.as_str()]
    );
    assert!(outcome.completion.unwrap().result().is_error);
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
    let mcp_call = call("local_run", json!({"command": command}));
    let started = start_call(&runtime, &results, &mcp_call).await;
    let (id, _) = approval_query(&started.messages);
    let outcome = respond(&runtime, &results, &answer(id, "run-once")).await;
    assert!(outcome.completion.is_none());
    let completion = tokio::time::timeout(std::time::Duration::from_secs(5), receiver.recv())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(completion.result().call_id, mcp_call.call_id);
    assert!(completion
        .result()
        .content
        .contains("cursor-devtools-local"));
    assert!(matches!(
        completion.tool_call().tool,
        Some(pb::tool_call::Tool::McpToolCall(_))
    ));
}

async fn stored_store(language: &str) -> crate::store::Store {
    let store = crate::store::Store::connect("sqlite::memory:")
        .await
        .unwrap();
    store.set_desktop_language(language).await.unwrap();
    store
}

// The user answers the approval in Cursor, so it follows the desktop app's
// interface language rather than staying English for everyone.
#[tokio::test]
async fn the_approval_question_follows_the_desktop_interface_language() {
    for (language, title, prompt_start, approve) in [
        (
            "zh-CN",
            "要在这台电脑上运行命令吗？",
            "电脑目录：",
            "运行一次",
        ),
        (
            "en-US",
            "Run a command on this desktop?",
            "Desktop directory:",
            "Run once",
        ),
    ] {
        let runtime = CursorToolRuntime::default();
        let (results, _receiver) = crate::cursor::tools::tool_call_result::tool_result_channel();
        let store = stored_store(language).await;
        let mcp_call = call("local_run", json!({"command":"printf localized"}));
        let started = start_with_store(&runtime, &results, Some(&store), &mcp_call).await;
        let (id, ask) = approval_query(&started.messages);
        let args = ask.args.as_ref().unwrap();
        assert_eq!(args.title, title, "{language}");
        assert!(
            args.questions[0].prompt.starts_with(prompt_start),
            "{language}: {}",
            args.questions[0].prompt
        );
        assert_eq!(args.questions[0].options[0].label, approve, "{language}");
        let outcome =
            respond_with_store(&runtime, &results, Some(&store), &answer(id, "run-once")).await;
        assert_eq!(
            completed_ids(&outcome.messages),
            [ask.tool_call_id.as_str()],
            "{language}"
        );
    }
}

// Without a store there is no language to follow: the question is English.
#[tokio::test]
async fn the_approval_question_is_english_without_a_store() {
    let runtime = CursorToolRuntime::default();
    let (results, _receiver) = crate::cursor::tools::tool_call_result::tool_result_channel();
    let mcp_call = call("local_run", json!({"command":"printf default-language"}));
    let started = start_call(&runtime, &results, &mcp_call).await;
    let (_, ask) = approval_query(&started.messages);
    assert_eq!(
        ask.args.as_ref().unwrap().title,
        "Run a command on this desktop?"
    );
}
