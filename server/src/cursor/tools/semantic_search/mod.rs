//! Runs SemanticSearch on the machine that holds the code, through Cursor's Exec channel.
//!
//! Search → (program or model absent) → deliver the parts → install and search.

mod command;
mod package;
mod result;

use std::collections::VecDeque;

use tokio::sync::OwnedMutexGuard;

use crate::{cursor::protocol::proto::agent::v1 as pb, model::ToolCall, Result};

use super::{
    codec::{self, ClientExecEvent},
    runtime::{now_ms, CursorToolRuntime, ExecContext, PendingExec},
    tool_call_result::ToolCompletion,
};
use command::{Installed, Missing};

pub use package::SemanticPackage;
pub(crate) use result::failed;

const DEFAULT_TOP_K: u64 = 5;
const MAX_TOP_K: u64 = 20;

/// Held by the one call that is installing; calls beside it wait and look again.
type InstallTurn = OwnedMutexGuard<()>;

pub(crate) enum SearchStage {
    Search(Option<InstallTurn>),
    Deliver(Install, VecDeque<(String, Vec<u8>)>),
    InstallAndSearch(Install),
}

pub(crate) struct Install {
    _turn: InstallTurn,
    files: Vec<Installed>,
}

/// A terminal answer from Cursor to one step of a search.
pub struct SearchReply {
    entry: PendingExec,
    answer: Answer,
    /// Stands for the call while no Exec does, so interrupting the run ends it.
    slot: u32,
}

enum Answer {
    Exited { code: u32, aborted: bool },
    Written,
    Refused(String),
}

pub(crate) enum Step {
    Send(Box<pb::AgentServerMessage>),
    Complete(Box<ToolCompletion>),
    Interrupted,
}

pub(crate) async fn start(
    runtime: &CursorToolRuntime,
    package: &SemanticPackage,
    call: &ToolCall,
    context: &ExecContext,
) -> Result<std::result::Result<pb::AgentServerMessage, ToolCompletion>> {
    let started_at_ms = now_ms();
    let stage = SearchStage::Search(None);
    let outcome = match supported(context) {
        Ok(()) => exec_message(&stage, package, call, context),
        Err(error) => Err(error),
    };
    match outcome {
        Ok(message) => {
            let id = runtime
                .reserve_search(call, context, stage, started_at_ms)
                .await?;
            Ok(Ok(codec::exec_message(id, call, message)))
        }
        Err(error) => Ok(Err(failed(call, started_at_ms, error)?)),
    }
}

/// Takes one Exec answer of a search. Nothing here blocks the conversation:
/// preparing the next step, which may download, happens in [`advance`].
pub(crate) async fn receive(
    id: u32,
    message: &pb::exec_client_message::Message,
    runtime: &CursorToolRuntime,
) -> Result<ClientExecEvent> {
    use pb::{exec_client_message::Message, shell_stream::Event, write_result::Result as Write};

    let answer = match message {
        Message::ShellStream(stream) => match stream.event.as_ref() {
            // The command is hidden, so its output never becomes a card update.
            Some(Event::Stdout(value)) => {
                runtime.append_stdout(id, &value.data).await;
                return Ok(ClientExecEvent::Pending);
            }
            Some(Event::Stderr(value)) => {
                runtime.append_stderr(id, &value.data).await;
                return Ok(ClientExecEvent::Pending);
            }
            Some(Event::Start(_)) | Some(Event::HookContext(_)) | None => {
                return Ok(ClientExecEvent::Pending)
            }
            Some(Event::Exit(exit)) => Answer::Exited {
                code: exit.code,
                aborted: exit.aborted,
            },
            Some(Event::Rejected(value)) => {
                Answer::Refused(format!("the command was rejected: {}", value.reason))
            }
            Some(Event::PermissionDenied(value)) => {
                Answer::Refused(format!("permission denied: {}", value.error))
            }
            Some(Event::SandboxUnsupported(value)) => {
                Answer::Refused(format!("the sandbox refused it: {}", value.reason))
            }
            Some(Event::Backgrounded(_)) => {
                Answer::Refused("the command was moved to the background".into())
            }
        },
        Message::WriteResult(result) => match result.result.as_ref() {
            Some(Write::Success(_)) => Answer::Written,
            Some(Write::Rejected(value)) => {
                Answer::Refused(format!("the write was rejected: {}", value.reason))
            }
            Some(Write::PermissionDenied(value)) => {
                Answer::Refused(format!("permission denied: {}", value.error))
            }
            Some(Write::NoSpace(_)) => Answer::Refused("no space left on the device".into()),
            Some(Write::Error(value)) => Answer::Refused(value.error.clone()),
            None => Answer::Refused("the write returned no result".into()),
        },
        _ => Answer::Refused("Cursor returned an unexpected result".into()),
    };
    let Some(call) = runtime.exec_call(id).await else {
        return Ok(ClientExecEvent::Pending);
    };
    let slot = runtime.reserve_interaction(&call, "").await?;
    let Some(entry) = runtime.take_exec(id).await else {
        runtime.discard_interaction(slot).await;
        return Ok(ClientExecEvent::Pending);
    };
    Ok(ClientExecEvent::Search(Box::new(SearchReply {
        entry,
        answer,
        slot,
    })))
}

/// Decides what follows an answer.
pub(crate) async fn advance(
    reply: SearchReply,
    runtime: &CursorToolRuntime,
    package: &SemanticPackage,
) -> Result<Step> {
    let SearchReply {
        entry,
        answer,
        slot,
    } = reply;
    let PendingExec {
        call,
        context,
        started_at_ms,
        stdout,
        stderr,
        stage,
    } = entry;
    let next = match stage.into_search() {
        Some(stage) => next(stage, answer, &stdout, &stderr, runtime, package).await,
        None => Err("SemanticSearch lost its place".into()),
    };
    let outcome = match next {
        Ok(Next::Found(found)) => Ok(Err(found)),
        Ok(Next::Stage(stage)) => {
            exec_message(&stage, package, &call, &context).map(|message| Ok((stage, message)))
        }
        Err(error) => Err(error),
    };
    let (stage, message) = match outcome {
        Ok(Ok(next)) => next,
        finished => {
            if runtime.take_interaction(slot).await.is_none() {
                return Ok(Step::Interrupted);
            }
            let completion = match finished {
                Ok(Err(found)) => result::found(&call, started_at_ms, found)?,
                Err(error) => failed(&call, started_at_ms, error)?,
                Ok(Ok(_)) => unreachable!("handled above"),
            };
            return Ok(Step::Complete(Box::new(completion)));
        }
    };
    // Reserved before the slot is given up, so an interrupt always finds one of them.
    let id = runtime
        .reserve_search(&call, &context, stage, started_at_ms)
        .await?;
    if runtime.take_interaction(slot).await.is_none() {
        runtime.discard_exec(id).await;
        return Ok(Step::Interrupted);
    }
    Ok(Step::Send(Box::new(codec::exec_message(
        id, &call, message,
    ))))
}

enum Next {
    Found(result::Found),
    Stage(SearchStage),
}

async fn next(
    stage: SearchStage,
    answer: Answer,
    stdout: &str,
    stderr: &str,
    runtime: &CursorToolRuntime,
    package: &SemanticPackage,
) -> std::result::Result<Next, String> {
    let code = match (&stage, answer) {
        (_, Answer::Refused(reason)) => return Err(format!("SemanticSearch failed: {reason}")),
        (_, Answer::Exited { aborted: true, .. }) => {
            return Err("SemanticSearch was stopped before it finished".into())
        }
        (SearchStage::Deliver(..), Answer::Written) => None,
        (
            SearchStage::Search(_) | SearchStage::InstallAndSearch(_),
            Answer::Exited { code, .. },
        ) => Some(code),
        _ => return Err("SemanticSearch received an answer to a different step".into()),
    };
    match (stage, code) {
        (SearchStage::Deliver(install, mut parts), _) => {
            parts.pop_front();
            Ok(Next::Stage(match parts.is_empty() {
                true => SearchStage::InstallAndSearch(install),
                false => SearchStage::Deliver(install, parts),
            }))
        }
        (_, Some(0)) => result::parse(stdout).map(Next::Found),
        (SearchStage::Search(turn), Some(command::MISSING)) => {
            let missing = command::parse_missing(stdout)?;
            let turn = match turn {
                Some(turn) => turn,
                None => match runtime.try_install_turn() {
                    Some(turn) => turn,
                    // Another call is installing; what is missing now may not be then.
                    None => {
                        let turn = runtime.install_turn().await;
                        return Ok(Next::Stage(SearchStage::Search(Some(turn))));
                    }
                },
            };
            deliver(turn, missing, package).await.map(Next::Stage)
        }
        (_, code) => {
            let detail = [stderr.trim(), stdout.trim()]
                .into_iter()
                .find(|text| !text.is_empty())
                .unwrap_or("no output");
            Err(format!(
                "SemanticSearch exited with code {}: {detail}",
                code.unwrap_or_default()
            ))
        }
    }
}

async fn deliver(
    turn: InstallTurn,
    missing: Missing,
    package: &SemanticPackage,
) -> std::result::Result<SearchStage, String> {
    let deliveries = package
        .deliveries(missing.platform, &missing.files)
        .await
        .map_err(|error| format!("SemanticSearch could not be installed: {error}"))?;
    let mut files = Vec::new();
    let mut parts = VecDeque::new();
    for delivery in deliveries {
        files.push(delivery.installed);
        parts.extend(delivery.parts);
    }
    Ok(SearchStage::Deliver(Install { _turn: turn, files }, parts))
}

fn exec_message(
    stage: &SearchStage,
    package: &SemanticPackage,
    call: &ToolCall,
    context: &ExecContext,
) -> std::result::Result<pb::exec_server_message::Message, String> {
    let hidden = |command| codec::hidden_shell(call, context, command);
    match stage {
        SearchStage::Search(_) => {
            command::search(&package.installation(), &request(call, context)?).map(hidden)
        }
        SearchStage::InstallAndSearch(install) => command::install_and_search(
            &package.installation(),
            &request(call, context)?,
            &parts_folder(context)?,
            &install.files,
        )
        .map(hidden),
        SearchStage::Deliver(_, parts) => {
            let (name, bytes) = parts
                .front()
                .ok_or("SemanticSearch has no part left to deliver")?;
            let path = format!("{}/{name}", parts_folder(context)?);
            Ok(codec::file_write(call, path, bytes.clone()))
        }
    }
}

/// Cursor lets an agent write under its project folder's `assets` without asking.
fn parts_folder(context: &ExecContext) -> std::result::Result<String, String> {
    match context.project_folder.trim_end_matches('/') {
        "" => Err("SemanticSearch cannot be installed: Cursor named no project folder".into()),
        folder => Ok(format!("{folder}/assets/byok-semantic")),
    }
}

fn supported(context: &ExecContext) -> std::result::Result<(), String> {
    if context.os_version.to_ascii_lowercase().starts_with("win") {
        return Err(
            "SemanticSearch does not run on Windows workspaces. Use Grep and Glob here.".into(),
        );
    }
    Ok(())
}

fn request(call: &ToolCall, context: &ExecContext) -> std::result::Result<String, String> {
    let text = |name: &str| {
        call.arguments
            .get(name)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    };
    let query = text("query").ok_or("SemanticSearch is missing query")?;
    let root = match (text("target_directory"), context.workspace_paths.as_slice()) {
        (Some(directory), _) if directory.starts_with('/') => directory,
        (Some(directory), _) => {
            return Err(format!(
                "SemanticSearch target_directory must be an absolute path, got `{directory}`"
            ))
        }
        (None, [root]) => root,
        (None, []) => {
            return Err("SemanticSearch needs target_directory: no workspace is open".into())
        }
        (None, roots) => {
            return Err(format!(
                "SemanticSearch needs target_directory: this workspace has several roots: {}",
                roots.join(", ")
            ))
        }
    };
    let top_k = match call.arguments.get("top_k") {
        None | Some(serde_json::Value::Null) => DEFAULT_TOP_K,
        Some(value) => value
            .as_u64()
            .filter(|value| (1..=MAX_TOP_K).contains(value))
            .ok_or_else(|| format!("SemanticSearch top_k must be 1 to {MAX_TOP_K}, got {value}"))?,
    };
    Ok(command::request(query, root, top_k))
}
