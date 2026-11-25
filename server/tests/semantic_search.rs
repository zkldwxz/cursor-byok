//! Verifies SemanticSearch over the Exec channel: search, first install, refusal and interruption.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io::{Read, Write},
    path::Path,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

use cursor_server::{
    cursor::{
        protocol::proto::agent::v1 as pb,
        tools::{
            codec::{self, ClientExecEvent},
            runtime::{CursorToolRuntime, ExecContext},
            semantic_search::SemanticPackage,
            ToolBatchState, ToolDispatcher,
        },
    },
    model::ToolCall,
};
use pb::{exec_client_message::Message as Client, exec_server_message::Message as Server};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;

const MODEL: &[u8] = b"weights";
const VSIX: &str = "byok-workspace-symbols-1.2.0.vsix";

fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn vsix() -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    writer
        .start_file(
            "extension/bin/linux-x64/byok-semantic",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
    writer.write_all(b"linux program").unwrap();
    writer.finish().unwrap().into_inner()
}

/// Serves the release and the model; counts what is asked of it.
fn serve() -> (String, Arc<AtomicUsize>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut buffer = [0; 4096];
            let read = stream.read(&mut buffer).unwrap();
            let request = String::from_utf8_lossy(&buffer[..read]).to_string();
            let path = request.split_whitespace().nth(1).unwrap_or("/");
            counter.fetch_add(1, Ordering::SeqCst);
            let (status, body) = if path == format!("/v1.2.0/{VSIX}") {
                ("200 OK", vsix())
            } else if path == format!("/v1.2.0/{VSIX}.sha256") {
                ("200 OK", format!("{}  {VSIX}\n", digest(&vsix())).into())
            } else if path == "/model/model.safetensors" {
                ("200 OK", MODEL.to_vec())
            } else {
                ("404 Not Found", Vec::new())
            };
            let head = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(head.as_bytes()).unwrap();
            stream.write_all(&body).unwrap();
        }
    });
    (url, hits)
}

struct Round {
    runtime: CursorToolRuntime,
    dispatcher: ToolDispatcher,
    results: cursor_server::cursor::tools::ToolResultReceiver,
    sender: mpsc::UnboundedSender<pb::AgentServerMessage>,
    sent: mpsc::UnboundedReceiver<pb::AgentServerMessage>,
    context: ExecContext,
}

impl Round {
    fn new(url: &str, cache: &Path) -> Self {
        let runtime = CursorToolRuntime::default();
        let package = SemanticPackage::with_sources(
            "1.2.0",
            url.into(),
            format!("{url}/model"),
            vec![("model.safetensors".into(), digest(MODEL))],
            cache.into(),
        );
        let (dispatcher, results) = ToolDispatcher::observed(runtime.clone());
        let (sender, sent) = mpsc::unbounded_channel();
        Self {
            runtime,
            dispatcher: dispatcher.with_semantic(package),
            results,
            sender,
            sent,
            context: ExecContext {
                conversation_id: "conversation".into(),
                os_version: "linux 6.8".into(),
                project_folder: "/home/dev/.cursor/projects/repo".into(),
                workspace_paths: vec!["/repo".into()],
                mcp_routes: HashMap::new(),
                ..Default::default()
            },
        }
    }

    async fn start(
        &self,
        id: &str,
        arguments: Value,
    ) -> cursor_server::cursor::tools::DispatchedTool {
        let call = ToolCall {
            index: 0,
            call_id: id.into(),
            model_call_id: "model:0".into(),
            name: "SemanticSearch".into(),
            arguments_text: arguments.to_string(),
            arguments,
            argument_error: None,
        };
        self.dispatcher
            .start_batch(
                &[call],
                ToolBatchState {
                    completed: &HashSet::new(),
                    started: &HashSet::new(),
                    response_text: "",
                    response_thinking: "",
                },
                &[],
                &BTreeMap::new(),
                &self.context,
            )
            .await
            .unwrap()
            .remove(0)
    }

    async fn search(&self, id: &str) -> pb::ExecServerMessage {
        let started = self.start(id, json!({"query": "verify token"})).await;
        assert!(started.completion.is_none());
        exec(started.messages.last().unwrap())
    }

    async fn event(&self, id: u32, message: Client) -> ClientExecEvent {
        codec::client_event(
            &pb::ExecClientMessage {
                id,
                message: Some(message),
                ..Default::default()
            },
            &self.runtime,
        )
        .await
        .unwrap()
    }

    /// Feeds a terminal answer and lets the search continue.
    async fn answer(&self, id: u32, message: Client) {
        let ClientExecEvent::Search(reply) = self.event(id, message).await else {
            panic!("a terminal answer must continue the search")
        };
        let sender = self.sender.clone();
        self.dispatcher.continue_search(*reply, move |message| {
            let _ = sender.send(message);
        });
    }

    async fn exit(&self, id: u32, code: u32, stdout: &str) {
        let event = self
            .event(
                id,
                stream(pb::shell_stream::Event::Stdout(pb::ShellStreamStdout {
                    data: stdout.into(),
                })),
            )
            .await;
        assert!(
            matches!(event, ClientExecEvent::Pending),
            "the hidden command must not stream into a card"
        );
        self.answer(
            id,
            stream(pb::shell_stream::Event::Exit(pb::ShellStreamExit {
                code,
                ..Default::default()
            })),
        )
        .await;
    }

    async fn written(&self, id: u32) {
        self.answer(
            id,
            Client::WriteResult(pb::WriteResult {
                result: Some(pb::write_result::Result::Success(
                    pb::WriteSuccess::default(),
                )),
            }),
        )
        .await;
    }

    async fn next_exec(&mut self) -> pb::ExecServerMessage {
        exec(&within(self.sent.recv()).await.unwrap())
    }

    async fn completion(&mut self) -> cursor_server::cursor::tools::ToolCompletion {
        within(self.results.recv()).await.unwrap().unwrap()
    }
}

async fn within<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(std::time::Duration::from_secs(20), future)
        .await
        .expect("the search stalled")
}

fn stream(event: pb::shell_stream::Event) -> Client {
    Client::ShellStream(pb::ShellStream { event: Some(event) })
}

fn exec(message: &pb::AgentServerMessage) -> pb::ExecServerMessage {
    match message.message.clone() {
        Some(pb::agent_server_message::Message::ExecServerMessage(exec)) => exec,
        other => panic!("expected an Exec, got {other:?}"),
    }
}

fn shell(exec: &pb::ExecServerMessage) -> pb::ShellArgs {
    match exec.message.clone() {
        Some(Server::ShellStreamArgs(args)) => args,
        other => panic!("expected a command, got {other:?}"),
    }
}

fn write(exec: &pb::ExecServerMessage) -> pb::WriteArgs {
    match exec.message.clone() {
        Some(Server::WriteArgs(args)) => args,
        other => panic!("expected a write, got {other:?}"),
    }
}

fn located() -> String {
    json!({"results": [{
        "path": "/repo/src/auth.rs", "start_line": 3, "end_line": 9,
        "score": 0.5, "content": "fn verify_token() {}"
    }]})
    .to_string()
}

#[tokio::test]
async fn an_installed_search_runs_one_unattended_command_and_answers_with_locations() {
    let cache = tempfile::tempdir().unwrap();
    let mut round = Round::new("http://127.0.0.1:1", cache.path());
    let query = round.search("call").await;
    let args = shell(&query);
    assert!(args.skip_approval);
    assert!(args.close_stdin);
    assert_eq!(
        args.timeout_behavior,
        pb::TimeoutBehavior::Cancel as i32,
        "a hidden command must never become a background terminal"
    );
    assert!(!args.command.contains("verify token"));

    round.exit(query.id, 0, &format!("{}\n", located())).await;
    let completion = round.completion().await;
    assert!(!completion.result().is_error);
    assert!(completion
        .result()
        .content
        .contains("/repo/src/auth.rs:3-9"));
    let Some(pb::tool_call::Tool::SemSearchToolCall(card)) = &completion.tool_call().tool else {
        panic!("expected the native search card")
    };
    assert_eq!(card.args.as_ref().unwrap().query, "verify token");
    let Some(pb::sem_search_tool_result::Result::Success(success)) = card
        .result
        .as_ref()
        .and_then(|result| result.result.as_ref())
    else {
        panic!("expected a successful card")
    };
    assert_eq!(success.code_results.len(), 1);
}

#[tokio::test]
async fn a_machine_without_the_program_gets_it_and_then_answers_the_same_search() {
    let (url, hits) = serve();
    let cache = tempfile::tempdir().unwrap();
    let mut round = Round::new(&url, cache.path());
    let query = round.search("call").await;
    round
        .exit(query.id, 97, "Linux x86_64 program model.safetensors\n")
        .await;
    for name in ["program", "model.safetensors"] {
        let delivery = round.next_exec().await;
        let args = write(&delivery);
        assert!(
            args.path.starts_with(&format!(
                "/home/dev/.cursor/projects/repo/assets/byok-semantic/{name}-"
            )),
            "{}",
            args.path
        );
        assert!(!args.file_bytes.is_empty());
        round.written(delivery.id).await;
    }
    let install = round.next_exec().await;
    let command = shell(&install).command;
    assert!(command.contains(&digest(b"linux program")));
    assert!(command.contains(&digest(MODEL)));
    round.exit(install.id, 0, &located()).await;
    assert!(!round.completion().await.result().is_error);
    assert_eq!(hits.load(Ordering::SeqCst), 3);

    // The next machine is served from the local copy.
    let mut second = Round::new(&url, cache.path());
    let query = second.search("call").await;
    second.exit(query.id, 97, "Linux x86_64 program").await;
    write(&second.next_exec().await);
    assert_eq!(hits.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn a_failed_install_is_reported_instead_of_retried() {
    let (url, _) = serve();
    let cache = tempfile::tempdir().unwrap();
    let mut round = Round::new(&url, cache.path());
    let query = round.search("call").await;
    round.exit(query.id, 97, "Linux x86_64 program").await;
    let delivery = round.next_exec().await;
    round.written(delivery.id).await;
    let install = round.next_exec().await;
    round.exit(install.id, 97, "Linux x86_64 program").await;
    let completion = round.completion().await;
    assert!(completion.result().is_error);
    assert!(completion.result().content.contains("97"));
    assert!(round.sent.try_recv().is_err());
}

#[tokio::test]
async fn a_download_failure_names_the_address_that_failed() {
    let (url, _) = serve();
    let cache = tempfile::tempdir().unwrap();
    let mut round = Round::new(&format!("{url}/elsewhere"), cache.path());
    let query = round.search("call").await;
    round.exit(query.id, 97, "Linux x86_64 program").await;
    let completion = round.completion().await;
    assert!(completion.result().is_error);
    assert!(
        completion
            .result()
            .content
            .contains(&format!("{url}/elsewhere")),
        "{}",
        completion.result().content
    );
}

#[tokio::test]
async fn a_refused_command_or_write_ends_the_search_with_its_reason() {
    let (url, _) = serve();
    let cache = tempfile::tempdir().unwrap();
    let mut round = Round::new(&url, cache.path());
    let query = round.search("refused").await;
    round
        .answer(
            query.id,
            stream(pb::shell_stream::Event::Rejected(pb::ShellRejected {
                reason: "blocked by the administrator".into(),
                ..Default::default()
            })),
        )
        .await;
    let completion = round.completion().await;
    assert!(completion.result().is_error);
    assert!(completion
        .result()
        .content
        .contains("blocked by the administrator"));

    let query = round.search("unwritable").await;
    round.exit(query.id, 97, "Linux x86_64 program").await;
    let delivery = round.next_exec().await;
    round
        .answer(
            delivery.id,
            Client::WriteResult(pb::WriteResult {
                result: Some(pb::write_result::Result::Rejected(pb::WriteRejected {
                    reason: "outside the workspace".into(),
                    ..Default::default()
                })),
            }),
        )
        .await;
    let completion = round.completion().await;
    assert!(completion
        .result()
        .content
        .contains("outside the workspace"));

    // The failed install gave its turn back.
    let query = round.search("again").await;
    round.exit(query.id, 97, "Linux x86_64 program").await;
    write(&round.next_exec().await);
}

#[tokio::test]
async fn searches_beside_an_install_wait_for_it_and_look_again() {
    let (url, _) = serve();
    let cache = tempfile::tempdir().unwrap();
    let mut round = Round::new(&url, cache.path());
    let first = round.search("first").await;
    let second = round.search("second").await;
    round.exit(first.id, 97, "Linux x86_64 program").await;
    let delivery = round.next_exec().await;
    round.exit(second.id, 97, "Linux x86_64 program").await;
    round.written(delivery.id).await;
    let install = round.next_exec().await;
    assert_eq!(install.exec_id, "first");
    assert!(round.sent.try_recv().is_err(), "the second call must wait");
    round.exit(install.id, 0, &located()).await;
    assert_eq!(round.completion().await.result().call_id, "first");

    let again = round.next_exec().await;
    assert_eq!(again.exec_id, "second");
    round.exit(again.id, 0, &located()).await;
    assert_eq!(round.completion().await.result().call_id, "second");
}

#[tokio::test]
async fn interrupting_the_run_stops_a_search_between_two_steps() {
    let (url, _) = serve();
    let cache = tempfile::tempdir().unwrap();
    let mut round = Round::new(&url, cache.path());
    let query = round.search("call").await;
    let ClientExecEvent::Search(reply) = round
        .event(
            query.id,
            stream(pb::shell_stream::Event::Exit(pb::ShellStreamExit {
                code: 0,
                ..Default::default()
            })),
        )
        .await
    else {
        panic!("expected the search to continue")
    };
    assert!(
        round.runtime.interrupt_for_message().await.is_empty(),
        "Cursor runs nothing for this call, so there is nothing to abort"
    );
    let sender = round.sender.clone();
    round.dispatcher.continue_search(*reply, move |message| {
        let _ = sender.send(message);
    });
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(round.sent.try_recv().is_err());
    assert!(round.runtime.running_exec_ids().await.is_empty());
    let silent = tokio::time::timeout(std::time::Duration::from_millis(50), round.results.recv());
    assert!(silent.await.is_err(), "an interrupted call has no result");
}

#[tokio::test]
async fn a_closed_stream_fails_the_search() {
    let cache = tempfile::tempdir().unwrap();
    let round = Round::new("http://127.0.0.1:1", cache.path());
    let query = round.search("call").await;
    let completion = codec::stream_closed(query.id, &round.runtime)
        .await
        .unwrap()
        .expect("the call must end");
    assert!(completion.result().is_error);
}

#[tokio::test]
async fn searches_that_cannot_run_say_why_before_any_command() {
    let cache = tempfile::tempdir().unwrap();
    let mut round = Round::new("http://127.0.0.1:1", cache.path());
    let mut refused = Vec::new();
    for arguments in [
        json!({}),
        json!({"query": "a", "target_directory": "src"}),
        json!({"query": "a", "top_k": 0}),
    ] {
        refused.push(round.start("call", arguments).await);
    }
    round.context.workspace_paths = vec!["/one".into(), "/two".into()];
    refused.push(round.start("call", json!({"query": "a"})).await);
    round.context.os_version = "win32 10.0.26100".into();
    refused.push(
        round
            .start("call", json!({"query": "a", "target_directory": "/one"}))
            .await,
    );
    for (dispatched, expected) in refused.iter().zip([
        "missing query",
        "absolute path",
        "top_k",
        "/one, /two",
        "Windows",
    ]) {
        let result = dispatched.completion.as_ref().expect(expected).result();
        assert!(result.is_error);
        assert!(result.content.contains(expected), "{}", result.content);
        assert!(dispatched.messages.len() <= 1, "no command may be sent");
    }
}

#[test]
fn every_mode_tells_the_model_when_semantic_search_comes_first() {
    use cursor_server::cursor::prompting::{Mode, PromptAssets};

    let assets = PromptAssets::load(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("prompt/cursor")
            .as_path(),
    )
    .unwrap();
    for mode in [
        Mode::Agent,
        Mode::Ask,
        Mode::Plan,
        Mode::Debug,
        Mode::Multitask,
        Mode::Subagent,
    ] {
        let mode = assets.mode(mode);
        let section = mode
            .prompt
            .split_once("<code_search>")
            .and_then(|(_, rest)| rest.split_once("</code_search>"))
            .expect("each prompt explains how to find code")
            .0;
        // Each tool keeps its own job; a guide that names only one hides the others.
        for tool in ["SemanticSearch", "Grep", "Read", "Shell"] {
            assert!(section.contains(tool), "{tool}");
        }
        for tool in &mode.tools {
            let steers = match tool.name.as_str() {
                "Shell" | "Task" => tool.description.as_str(),
                _ => continue,
            };
            assert!(steers.contains("SemanticSearch"), "{}", tool.name);
        }
    }
}
