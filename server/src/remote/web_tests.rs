use std::collections::HashMap;

use futures_util::SinkExt;
use prost::Message as _;
use tokio::{net::TcpListener, net::TcpStream, sync::mpsc};
use tokio_tungstenite::{
    tungstenite::{client::IntoClientRequest, Message as Frame},
    MaybeTlsStream, WebSocketStream,
};

use super::super::{
    app,
    relay::{testing::FakeCursor, Relay},
};
use super::*;
use crate::{cursor::transport::MessageModes, store::Store};

const WORKSPACE: &str = "/projects/shop";

struct Computer {
    relay: Relay,
    modes: MessageModes,
    access: Arc<Access>,
    phones: Phones,
    address: String,
    stop: CancellationToken,
}

impl Computer {
    async fn start() -> Self {
        let directory = tempfile::tempdir().unwrap().keep();
        let url = format!("sqlite://{}", directory.join("state.db").display());
        let store = Store::connect(&url).await.unwrap();
        let (relay, modes) = (Relay::default(), MessageModes::default());
        let stop = CancellationToken::new();
        let access = Arc::new(Access::new(stop.clone()));
        let phones = Phones::new(
            access.clone(),
            Sessions::new(relay.clone(), modes.clone()),
            Models::new(store, None),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap().to_string();
        tokio::spawn({
            let phones = phones.clone();
            async move { phones.follow().await }
        });
        let served = router(phones.clone());
        tokio::spawn(async move { axum::serve(listener, served).await.unwrap() });
        Self {
            relay,
            modes,
            access,
            phones,
            address,
            stop,
        }
    }

    fn token(&self) -> String {
        let code = self.access.new_pairing_code();
        self.access.pair(&code, "phone").unwrap()
    }

    async fn phone(&self) -> Phone {
        let mut phone = Phone::connect(&self.address, &self.address).await.unwrap();
        phone.say(json!({"token": self.token()})).await;
        phone
    }
}

struct Phone(WebSocketStream<MaybeTlsStream<TcpStream>>);

impl Phone {
    async fn connect(
        address: &str,
        origin: &str,
    ) -> std::result::Result<Self, tokio_tungstenite::tungstenite::Error> {
        let mut request = format!("ws://{address}/ws").into_client_request().unwrap();
        let origin = format!("http://{origin}").parse().unwrap();
        request.headers_mut().insert("origin", origin);
        Ok(Self(tokio_tungstenite::connect_async(request).await?.0))
    }

    async fn say(&mut self, message: Value) {
        self.0.send(Frame::text(message.to_string())).await.unwrap();
    }

    /// The next message that satisfies `wanted`.
    async fn hear(&mut self, wanted: impl Fn(&Value) -> bool) -> Value {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let frame = self.0.next().await.expect("the server hung up").unwrap();
                if let Frame::Text(text) = frame {
                    let message: Value = serde_json::from_str(&text).unwrap();
                    if wanted(&message) {
                        return message;
                    }
                }
            }
        })
        .await
        .expect("the message did not come")
    }

    async fn hear_type(&mut self, kind: &str) -> Value {
        self.hear(|message| message["type"] == kind).await
    }

    async fn command(&mut self, command: Value) -> Value {
        self.say(json!({"command": command})).await;
        self.hear(|message| {
            ["done", "refused", "menu"].contains(&message["type"].as_str().unwrap_or_default())
        })
        .await
    }
}

type Action = Box<dyn FnOnce(&FakeCursor) + Send>;

/// Answers the overview as Cursor does, and hands every other call to the test.
struct Agents {
    calls: mpsc::UnboundedReceiver<(String, String, Vec<u8>)>,
    send: mpsc::UnboundedSender<Action>,
    streams: HashMap<String, String>,
}

impl Agents {
    fn open(relay: &Relay, sessions: Vec<pb::AgentHostSession>) -> Self {
        let mut cursor = FakeCursor::connect(relay);
        let (call, calls) = mpsc::unbounded_channel();
        let (send, mut actions) = mpsc::unbounded_channel::<Action>();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    action = actions.recv() => match action {
                        Some(action) => action(&cursor),
                        None => return,
                    },
                    request = cursor.next() => {
                        let Some((id, method, data)) = request else { return };
                        let method = method.rsplit('/').next().unwrap().to_owned();
                        match method.as_str() {
                            "WatchSessions" => cursor.answer(&id, &pb::WatchAgentHostSessionsEvent {
                                event: Some(pb::watch_agent_host_sessions_event::Event::Snapshot(
                                    pb::AgentHostSessionsSnapshot { sessions: sessions.clone() },
                                )),
                            }),
                            "ListWorkspaces" => cursor.answer(&id, &pb::ListAgentHostWorkspacesResponse {
                                workspaces: vec![workspace()],
                            }),
                            "CancelRequest" => {}
                            _ => { let _ = call.send((id, method, data)); }
                        }
                    }
                }
            }
        });
        Self {
            calls,
            send,
            streams: HashMap::new(),
        }
    }

    async fn called<Request: prost::Message + Default>(
        &mut self,
        method: &str,
    ) -> (String, Request) {
        let call = tokio::time::timeout(Duration::from_secs(10), self.calls.recv());
        let (id, found, data) = call.await.expect("Cursor was not called").unwrap();
        assert_eq!(found, method);
        (id, Request::decode(data.as_slice()).unwrap())
    }

    /// Takes the phone's attach, so that `event` reaches it.
    async fn attached(&mut self, session: &str) {
        let (id, request) = self
            .called::<pb::AttachAgentHostSessionRequest>("AttachSession")
            .await;
        assert_eq!(request.session_id, session);
        self.streams.insert(session.into(), id);
    }

    fn answer(&self, id: &str, message: impl prost::Message + 'static) {
        let id = id.to_owned();
        let _ = self
            .send
            .send(Box::new(move |cursor| cursor.answer(&id, &message)));
    }

    fn event(&self, session: &str, event: Event) {
        self.answer(
            &self.streams[session],
            pb::AgentHostSessionEvent {
                event_id: 1,
                event: Some(event),
            },
        );
    }
}

fn workspace() -> pb::AgentHostWorkspace {
    pb::AgentHostWorkspace {
        workspace_paths: vec![WORKSPACE.into()],
        ..Default::default()
    }
}

fn session(id: &str) -> pb::AgentHostSession {
    pb::AgentHostSession {
        session_id: id.into(),
        title: Some(format!("about {id}")),
        status: pb::AgentHostSessionStatus::AwaitingInput as i32,
        model_id: Some("model-a".into()),
        workspace: Some(workspace()),
        ..Default::default()
    }
}

fn says(text: &str) -> Event {
    Event::InteractionUpdate(pb::InteractionUpdate {
        message: Some(pb::interaction_update::Message::TextDelta(
            pb::TextDeltaUpdate {
                text: text.into(),
                ..Default::default()
            },
        )),
    })
}

fn started() -> Event {
    Event::TurnStarted(pb::AgentHostTurnStarted {
        turn_id: "turn".into(),
    })
}

fn asks() -> Event {
    Event::TurnAwaitingInput(pb::AgentHostTurnAwaitingInput {
        turn_id: "turn".into(),
        interaction_id: "ask".into(),
        question: Some(pb::agent_host_turn_awaiting_input::Question::ExecApproval(
            pb::AgentHostApprovalRequest::default(),
        )),
        ..Default::default()
    })
}

#[tokio::test]
async fn only_the_page_itself_may_connect() {
    let computer = Computer::start().await;
    assert!(Phone::connect(&computer.address, "elsewhere.example")
        .await
        .is_err());
    assert!(Phone::connect(&computer.address, &computer.address)
        .await
        .is_ok());
}

#[tokio::test]
async fn a_device_that_is_not_paired_is_turned_away() {
    let computer = Computer::start().await;
    let mut phone = Phone::connect(&computer.address, &computer.address)
        .await
        .unwrap();
    phone.say(json!({"token": "guess"})).await;
    phone.hear_type("unpaired").await;
}

#[tokio::test]
async fn turning_remote_control_off_ends_the_session() {
    let computer = Computer::start().await;
    let mut phone = computer.phone().await;
    phone.hear_type("overview").await;
    computer.stop.cancel();
    phone.hear_type("unpaired").await;
}

#[tokio::test]
async fn the_overview_follows_cursor_as_it_comes_and_goes() {
    let computer = Computer::start().await;
    let mut phone = computer.phone().await;
    phone.hear(|it| it["reachable"] == false).await;
    let agents = Agents::open(&computer.relay, vec![session("one")]);
    let overview = phone
        .hear(|it| it["workspaces"][0]["chats"][0]["id"] == "one")
        .await;
    let shop = &overview["workspaces"][0];
    assert_eq!(
        (&shop["id"], &shop["title"]),
        (&json!(WORKSPACE), &json!("shop"))
    );
    assert_eq!(shop["chats"][0]["label"], "about one");
    assert_eq!(shop["chats"][0]["state"], "waiting");
    drop(agents);
    computer.relay.close();
    phone.hear(|it| it["reachable"] == false).await;
}

#[tokio::test]
async fn an_attached_conversation_starts_with_its_history_and_then_sends_only_what_changes() {
    let computer = Computer::start().await;
    let mut agents = Agents::open(&computer.relay, vec![session("one")]);
    let mut phone = computer.phone().await;
    phone.say(json!({"attach": "one"})).await;
    agents.attached("one").await;
    agents.event(
        "one",
        Event::ConversationState(pb::ConversationStateStructure {
            turns: vec![b"turn".to_vec()],
            mode: Some(pb::AgentMode::Plan as i32),
            ..Default::default()
        }),
    );
    let blob = |id: &[u8], data: Vec<u8>| pb::AgentHostSessionBlob {
        blob_id: id.to_vec(),
        data,
    };
    let (id, asked) = agents
        .called::<pb::GetAgentHostSessionBlobsRequest>("GetSessionBlobs")
        .await;
    assert_eq!(asked.blob_ids, vec![b"turn".to_vec()]);
    let turn = pb::ConversationTurnStructure {
        turn: Some(
            pb::conversation_turn_structure::Turn::AgentConversationTurn(
                pb::AgentConversationTurnStructure {
                    user_message: b"user".to_vec(),
                    ..Default::default()
                },
            ),
        ),
    };
    agents.answer(
        &id,
        pb::GetAgentHostSessionBlobsResponse {
            blobs: vec![blob(b"turn", turn.encode_to_vec())],
            ..Default::default()
        },
    );
    let (id, _) = agents
        .called::<pb::GetAgentHostSessionBlobsRequest>("GetSessionBlobs")
        .await;
    let user = pb::UserMessage {
        text: "earlier".into(),
        ..Default::default()
    };
    agents.answer(
        &id,
        pb::GetAgentHostSessionBlobsResponse {
            blobs: vec![blob(b"user", user.encode_to_vec())],
            ..Default::default()
        },
    );
    let chat = phone.hear(|it| it["rows"][0]["text"] == "earlier").await;
    assert_eq!((&chat["from"], &chat["mode"]), (&json!(0), &json!("plan")));
    agents.event("one", says("now"));
    let chat = phone
        .hear(|it| it["type"] == "chat" && it["from"] == 1)
        .await;
    assert_eq!(chat["rows"].as_array().unwrap().len(), 1);
    assert_eq!(chat["rows"][0]["blocks"][0]["runs"][0]["text"], "now");
}

#[tokio::test]
async fn a_new_conversation_begins_with_its_first_message_in_the_chosen_mode_and_model() {
    let computer = Computer::start().await;
    let mut agents = Agents::open(&computer.relay, Vec::new());
    let mut phone = computer.phone().await;
    phone
        .hear(|it| it["workspaces"][0]["id"] == WORKSPACE)
        .await;
    phone.say(json!({"draft": WORKSPACE})).await;
    for pick in [
        json!({"type": "pick", "kind": "mode", "id": "plan"}),
        json!({"type": "pick", "kind": "model", "id": "model-b"}),
    ] {
        assert_eq!(phone.command(pick).await["type"], "done");
    }
    phone
        .say(json!({"command": {"type": "send", "text": "hello"}}))
        .await;
    let (id, create) = agents
        .called::<pb::CreateAgentHostSessionRequest>("CreateSession")
        .await;
    assert_eq!(
        create.session_options.unwrap().workspace_paths,
        vec![WORKSPACE.to_owned()]
    );
    agents.answer(&id, pb::CreateAgentHostSessionResponse {});
    agents.attached(&create.session_id).await;
    let (id, message) = agents
        .called::<pb::SendAgentHostMessageRequest>("SendMessage")
        .await;
    assert_eq!(message.text, "hello");
    assert_eq!(message.requested_model.unwrap().model_id, "model-b");
    agents.answer(&id, pb::SendAgentHostMessageResponse::default());
    let done = phone.hear_type("done").await;
    assert_eq!(done["session"], json!(create.session_id));
    let chat = phone
        .hear(|it| it["type"] == "chat" && it["session"] == json!(create.session_id))
        .await;
    assert_eq!(chat["draft"], Value::Null);

    // What Cursor later asks this server to run carries no mode of its own.
    let mut run = pb::AgentClientMessage {
        message: Some(pb::agent_client_message::Message::RunRequest(
            pb::AgentRunRequest {
                action: Some(pb::ConversationAction {
                    action: Some(pb::conversation_action::Action::UserMessageAction(
                        pb::UserMessageAction {
                            user_message: Some(pb::UserMessage {
                                message_id: message.message_id.unwrap(),
                                ..Default::default()
                            }),
                            ..Default::default()
                        },
                    )),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )),
    };
    computer.modes.fill(&mut run);
    let text = format!("{run:?}");
    assert!(text.contains("mode: Plan"), "{text}");
}

#[tokio::test]
async fn a_conversation_keeps_its_model_until_another_is_picked() {
    let computer = Computer::start().await;
    let mut agents = Agents::open(&computer.relay, vec![session("one")]);
    let mut phone = computer.phone().await;
    phone.hear(|it| it["reachable"] == true).await;
    phone.say(json!({"attach": "one"})).await;
    agents.attached("one").await;
    for (pick, expected) in [(None, "model-a"), (Some("model-b"), "model-b")] {
        if let Some(id) = pick {
            phone
                .command(json!({"type": "pick", "kind": "model", "id": id}))
                .await;
        }
        phone
            .say(json!({"command": {"type": "send", "text": "go on"}}))
            .await;
        let (id, message) = agents
            .called::<pb::SendAgentHostMessageRequest>("SendMessage")
            .await;
        assert_eq!(message.requested_model.unwrap().model_id, expected);
        agents.answer(&id, pb::SendAgentHostMessageResponse::default());
        phone.hear_type("done").await;
    }
}

#[tokio::test]
async fn a_conversation_cannot_begin_where_cursor_has_no_workspace() {
    let computer = Computer::start().await;
    let _agents = Agents::open(&computer.relay, Vec::new());
    let mut phone = computer.phone().await;
    phone.hear(|it| it["reachable"] == true).await;
    phone.say(json!({"draft": "/etc"})).await;
    phone
        .command(json!({"type": "pick", "kind": "model", "id": "model-b"}))
        .await;
    let reply = phone
        .command(json!({"type": "send", "text": "hello"}))
        .await;
    assert_eq!(reply["code"], "gone");
}

#[tokio::test]
async fn an_approval_reaches_cursor_once_and_a_late_one_is_refused() {
    let computer = Computer::start().await;
    let mut agents = Agents::open(&computer.relay, vec![session("one")]);
    let mut phone = computer.phone().await;
    phone.say(json!({"attach": "one"})).await;
    agents.attached("one").await;
    agents.event("one", started());
    agents.event("one", asks());
    phone.hear(|it| it["asked"][0]["id"] == "ask").await;
    for (verdict, approved, status, reply) in [
        (
            "reject",
            false,
            pb::AgentHostInteractionResponseStatus::Accepted,
            "done",
        ),
        (
            "approve",
            true,
            pb::AgentHostInteractionResponseStatus::AlreadyResolved,
            "refused",
        ),
    ] {
        phone
            .say(json!({"command": {"type": "answer", "id": "ask", "verdict": verdict}}))
            .await;
        let (id, request) = agents
            .called::<pb::RespondToAgentHostInteractionRequest>("RespondToInteraction")
            .await;
        use pb::respond_to_agent_host_interaction_request::Answer;
        let Some(Answer::ExecApproval(result)) = request.answer else {
            panic!("not an approval");
        };
        assert_eq!(result.approved, approved);
        agents.answer(
            &id,
            pb::RespondToAgentHostInteractionResponse {
                status: status as i32,
            },
        );
        phone.hear_type(reply).await;
    }
}

#[tokio::test]
async fn stop_interrupts_the_running_turn_and_only_that() {
    let computer = Computer::start().await;
    let mut agents = Agents::open(&computer.relay, vec![session("one")]);
    let mut phone = computer.phone().await;
    phone.say(json!({"attach": "one"})).await;
    agents.attached("one").await;
    assert_eq!(phone.command(json!({"type": "stop"})).await["code"], "gone");
    agents.event("one", started());
    phone.hear(|it| it["running"] == true).await;
    phone.say(json!({"command": {"type": "stop"}})).await;
    let (id, request) = agents
        .called::<pb::InterruptAgentHostTurnRequest>("InterruptTurn")
        .await;
    assert_eq!(request.turn_id, "turn");
    agents.answer(
        &id,
        pb::InterruptAgentHostTurnResponse { interrupted: true },
    );
    phone.hear_type("done").await;
}

#[tokio::test]
async fn a_conversation_is_taken_up_again_after_cursor_reconnects() {
    let computer = Computer::start().await;
    let mut agents = Agents::open(&computer.relay, vec![session("one")]);
    let mut phone = computer.phone().await;
    phone.say(json!({"attach": "one"})).await;
    agents.attached("one").await;
    agents.event("one", says("before"));
    phone.hear(|it| it["rows"][0]["key"] == "1").await;
    drop(agents);
    computer.relay.close();
    let mut agents = Agents::open(&computer.relay, vec![session("one")]);
    agents.attached("one").await;
    agents.event("one", says("after"));
    phone
        .hear(|it| it["rows"][0]["blocks"][0]["runs"][0]["text"] == "after")
        .await;
}

async fn heard_by_app(link: &mut remote_link::Link, kind: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let text = link.receive().await.expect("the computer hung up");
            let message: Value = serde_json::from_str(&text).unwrap();
            if message["type"] == kind {
                return message;
            }
        }
    })
    .await
    .expect("the message did not come")
}

async fn app(computer: &Computer, hello: Value) -> (remote_link::Phone, remote_link::Link) {
    let host = remote_link::Host::bind(None).await.unwrap();
    let ticket = host.ticket(String::new());
    let phones = computer.phones.clone();
    tokio::spawn(async move { app::listen(&host, &phones).await });
    let phone = remote_link::Phone::bind(None).await.unwrap();
    let mut link = phone.dial(&ticket).await.unwrap();
    link.send(&hello.to_string()).await.unwrap();
    (phone, link)
}

#[tokio::test]
async fn an_app_pairs_once_and_then_follows_like_a_browser() {
    let computer = Computer::start().await;
    let _agents = Agents::open(&computer.relay, vec![session("one")]);
    let code = computer.access.new_pairing_code();
    let hello = json!({"code": code, "name": "Pixel"});
    let (_phone, mut link) = app(&computer, hello.clone()).await;
    let paired = heard_by_app(&mut link, "paired").await;
    heard_by_app(&mut link, "overview").await;
    assert_eq!(computer.access.devices()[0].name, "Pixel");
    let (_phone, mut again) = app(&computer, json!({"token": paired["token"]})).await;
    heard_by_app(&mut again, "overview").await;
    let (_phone, mut second) = app(&computer, hello).await;
    heard_by_app(&mut second, "unpaired").await;
    assert_eq!(computer.access.devices().len(), 1);
}

#[tokio::test]
async fn an_app_that_is_not_paired_hears_nothing_of_the_computer() {
    let computer = Computer::start().await;
    for hello in [
        json!({"token": "guess"}),
        json!({"code": "", "name": "x"}),
        json!({}),
    ] {
        let (_phone, mut link) = app(&computer, hello).await;
        let first: Value = serde_json::from_str(&link.receive().await.unwrap()).unwrap();
        assert_eq!(first, json!({"type": "unpaired"}));
        assert!(link.receive().await.is_none());
    }
}

#[tokio::test]
async fn removing_a_device_ends_its_app_session() {
    let computer = Computer::start().await;
    let (_phone, mut link) = app(&computer, json!({"token": computer.token()})).await;
    heard_by_app(&mut link, "overview").await;
    computer.access.remove(&computer.access.devices()[0].id);
    heard_by_app(&mut link, "unpaired").await;
}
