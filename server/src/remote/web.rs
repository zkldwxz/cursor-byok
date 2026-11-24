//! Serves paired phones and browsers: the page they load, pairing, and their live session.
use std::{pin::Pin, sync::Arc, time::Duration};

use axum::{
    extract::{
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    http::{header, HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use futures_util::{Stream, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::{
    cursor::protocol::proto::agent::v1::{self as pb, agent_host_session_event::Event},
    Error, Result,
};

use super::{
    access::Access,
    models::Models,
    sessions::{Overview, Sessions, Verdict},
    transcript::{Row, Transcript},
};

/// Often enough for text to flow, seldom enough for a slow connection.
const PUBLISH_INTERVAL: Duration = Duration::from_millis(40);
const REATTACH_INTERVAL: Duration = Duration::from_secs(2);
// Cloudflare drops connections that stay silent for 100 seconds.
const PING_INTERVAL: Duration = Duration::from_secs(30);
const AUTH_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_MESSAGE_BYTES: usize = 256 * 1024;

const HTML: &str = "text/html; charset=utf-8";
const CSS: &str = "text/css; charset=utf-8";
const JS: &str = "text/javascript; charset=utf-8";
const SVG: &str = "image/svg+xml; charset=utf-8";
const CLIENT: [(&str, &str, &str); 11] = [
    ("/", HTML, include_str!("client/index.html")),
    ("/style.css", CSS, include_str!("client/style.css")),
    ("/icon.svg", SVG, include_str!("client/icon.svg")),
    ("/app.js", JS, include_str!("client/app.js")),
    ("/link.js", JS, include_str!("client/link.js")),
    ("/model.js", JS, include_str!("client/model.js")),
    ("/dom.js", JS, include_str!("client/dom.js")),
    ("/copy.js", JS, include_str!("client/copy.js")),
    ("/sidebar.js", JS, include_str!("client/sidebar.js")),
    ("/chat.js", JS, include_str!("client/chat.js")),
    ("/dock.js", JS, include_str!("client/dock.js")),
];

#[derive(Clone)]
pub(super) struct Phones {
    access: Arc<Access>,
    sessions: Sessions,
    models: Models,
    overview: watch::Sender<Overview>,
}

impl Phones {
    pub fn new(access: Arc<Access>, sessions: Sessions, models: Models) -> Self {
        Self {
            access,
            sessions,
            models,
            overview: watch::channel(Overview::default()).0,
        }
    }

    /// One watch serves every phone.
    pub async fn follow(&self) {
        self.sessions.follow(&self.overview).await;
    }
}

/// Kept apart from the management router: a tunnel reaches everything here.
pub(super) fn router(phones: Phones) -> Router {
    let mut router = Router::new();
    for (path, content_type, body) in CLIENT {
        router = router.route(path, get(move || async move { asset(content_type, body) }));
    }
    router
        .route("/api/pair", post(pair))
        .route("/ws", get(connect))
        .with_state(phones)
}

fn asset(content_type: &'static str, body: &'static str) -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "no-store"),
            (header::REFERRER_POLICY, "no-referrer"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (
                header::CONTENT_SECURITY_POLICY,
                "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self'; connect-src 'self'; \
                 base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
            ),
        ],
        body,
    )
}

#[derive(Deserialize)]
struct PairRequest {
    code: String,
    name: String,
}

async fn pair(
    State(phones): State<Phones>,
    headers: HeaderMap,
    Json(request): Json<PairRequest>,
) -> Response {
    if !same_origin(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    match phones.access.pair(&request.code, &request.name) {
        Some(token) => (
            [(header::CACHE_CONTROL, "no-store")],
            Json(json!({"token": token})),
        )
            .into_response(),
        None => StatusCode::UNAUTHORIZED.into_response(),
    }
}

async fn connect(
    State(phones): State<Phones>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    if !same_origin(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    upgrade
        .max_message_size(MAX_MESSAGE_BYTES)
        .on_upgrade(move |socket| serve(socket, phones))
}

fn same_origin(headers: &HeaderMap) -> bool {
    let text = |name| headers.get(name).and_then(|value| value.to_str().ok());
    let origin = text(header::ORIGIN)
        .and_then(|origin| origin.parse::<Uri>().ok())
        .and_then(|origin| origin.authority().map(ToString::to_string));
    origin.is_some() && origin.as_deref() == text(header::HOST)
}

/// The token comes as a message, so it never appears in a URL.
async fn serve(mut socket: WebSocket, phones: Phones) {
    let Ok(Some(session)) =
        tokio::time::timeout(AUTH_TIMEOUT, authenticate(&mut socket, &phones)).await
    else {
        let _ = send(&mut socket, json!({"type": "unpaired"})).await;
        return;
    };
    tokio::select! {
        () = session.cancelled() => {
            let _ = send(&mut socket, json!({"type": "unpaired"})).await;
        }
        result = Session::new(&mut socket, &phones).run() => {
            if let Err(error) = result {
                tracing::debug!(%error, "phone session ended");
            }
        }
    }
}

async fn authenticate(socket: &mut WebSocket, phones: &Phones) -> Option<CancellationToken> {
    #[derive(Deserialize)]
    struct Hello {
        token: String,
    }
    let Message::Text(text) = socket.recv().await?.ok()? else {
        return None;
    };
    let hello: Hello = serde_json::from_str(&text).ok()?;
    phones.access.session(&hello.token)
}

async fn send(socket: &mut WebSocket, message: Value) -> Result<()> {
    socket
        .send(Message::text(message.to_string()))
        .await
        .map_err(|error| Error::Provider(format!("phone connection: {error}")))
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Request {
    Overview,
    Attach(String),
    Draft(String),
    Command(Command),
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Command {
    Send { text: String },
    Stop,
    Answer { id: String, verdict: Verdict },
    Menu { kind: Menu },
    Pick { kind: Menu, id: String },
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Menu {
    Mode,
    Model,
}

/// Cursor's modes this server has prompts for.
const MODES: [(&str, pb::AgentMode); 5] = [
    ("agent", pb::AgentMode::Agent),
    ("plan", pb::AgentMode::Plan),
    ("ask", pb::AgentMode::Ask),
    ("debug", pb::AgentMode::Debug),
    ("multitask", pb::AgentMode::Multitask),
];

/// Why a command was not carried out, as a code the phone has words for.
enum Refusal {
    Offline,
    /// Answered elsewhere, or no longer asked.
    Gone,
    NoModel,
    Failed(Error),
}

impl From<Error> for Refusal {
    fn from(error: Error) -> Self {
        Self::Failed(error)
    }
}

impl Refusal {
    fn code(&self) -> &'static str {
        match self {
            Self::Offline => "offline",
            Self::Gone => "gone",
            Self::NoModel => "no_model",
            Self::Failed(_) => "failed",
        }
    }
}

type Events = Pin<Box<dyn Stream<Item = Result<pb::AgentHostSessionEvent>> + Send>>;

/// The conversation on the phone's screen.
struct Chat {
    place: Place,
    transcript: Transcript,
    events: Option<Events>,
    /// Nothing has been heard yet: a saved state now is the conversation so far.
    fresh: bool,
    mode: Option<pb::AgentMode>,
    model: Option<String>,
    /// The model as the phone shows it.
    label: Option<String>,
    sent: Option<(Vec<Row>, Value)>,
}

enum Place {
    Session(String),
    /// A conversation not begun, in this workspace.
    Draft(String),
}

impl Chat {
    fn new(place: Place) -> Self {
        Self {
            place,
            transcript: Transcript::default(),
            events: None,
            fresh: true,
            mode: None,
            model: None,
            label: None,
            sent: None,
        }
    }

    fn session(&self) -> Option<&str> {
        match &self.place {
            Place::Session(id) => Some(id),
            Place::Draft(_) => None,
        }
    }

    fn mode(&self) -> pb::AgentMode {
        self.mode
            .or(self.transcript.mode())
            .unwrap_or(pb::AgentMode::Agent)
    }
}

struct Session<'a> {
    socket: &'a mut WebSocket,
    phones: &'a Phones,
    overview: watch::Receiver<Overview>,
    chat: Option<Chat>,
}

impl<'a> Session<'a> {
    fn new(socket: &'a mut WebSocket, phones: &'a Phones) -> Self {
        Self {
            socket,
            phones,
            overview: phones.overview.subscribe(),
            chat: None,
        }
    }

    async fn run(mut self) -> Result<()> {
        let mut publish = tokio::time::interval(PUBLISH_INTERVAL);
        publish.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut reattach = tokio::time::interval(REATTACH_INTERVAL);
        let mut ping = tokio::time::interval(PING_INTERVAL);
        self.list().await?;
        loop {
            tokio::select! {
                message = self.socket.recv() => match message {
                    Some(Ok(Message::Text(text))) => self.handle(&text).await?,
                    Some(Ok(Message::Close(_)) | Err(_)) | None => return Ok(()),
                    Some(Ok(_)) => {}
                },
                Ok(()) = self.overview.changed() => self.list().await?,
                event = heard(&mut self.chat) => self.hear(event).await,
                _ = publish.tick() => self.publish().await?,
                _ = reattach.tick() => self.reattach(),
                _ = ping.tick() => {
                    let ping = self.socket.send(Message::Ping(Default::default())).await;
                    if ping.is_err() {
                        return Ok(());
                    }
                }
            }
        }
    }

    async fn handle(&mut self, text: &str) -> Result<()> {
        match serde_json::from_str(text)? {
            Request::Overview => self.list().await,
            Request::Attach(id) => {
                self.chat = Some(Chat::new(Place::Session(id)));
                self.reattach();
                self.relabel().await
            }
            Request::Draft(workspace) => {
                self.chat = Some(Chat::new(Place::Draft(workspace)));
                self.relabel().await?;
                self.publish().await
            }
            Request::Command(command) => {
                let mut reply = match self.carry_out(command).await {
                    Ok(reply) => reply,
                    Err(refusal) => {
                        if let Refusal::Failed(error) = &refusal {
                            tracing::warn!(%error, "phone command failed");
                        }
                        json!({"type": "refused", "code": refusal.code()})
                    }
                };
                // A draft that became a session, even if its first message then failed.
                reply["session"] = json!(self.chat.as_ref().and_then(Chat::session));
                send(self.socket, reply).await?;
                self.relabel().await?;
                self.publish().await
            }
        }
    }

    async fn carry_out(&mut self, command: Command) -> Result<Value, Refusal> {
        if let Command::Menu { kind } = command {
            let items = match kind {
                Menu::Mode => json!(MODES.map(|(id, _)| json!({"id": id}))),
                Menu::Model => json!(self.phones.models.list().await?),
            };
            return Ok(json!({"type": "menu", "items": items}));
        }
        let chat = self.chat.as_mut().ok_or(Refusal::Offline)?;
        let sessions = &self.phones.sessions;
        match command {
            Command::Send { text } => self.say(text).await?,
            Command::Stop => {
                let turn = chat.transcript.running().ok_or(Refusal::Gone)?;
                let session = chat.session().ok_or(Refusal::Gone)?;
                sessions.stop(session, turn).await?;
            }
            Command::Answer { id, verdict } => {
                let session = chat.session().ok_or(Refusal::Gone)?;
                let asked = chat.transcript.pending(&id).ok_or(Refusal::Gone)?;
                if !sessions.respond(session, asked, verdict).await? {
                    return Err(Refusal::Gone);
                }
            }
            Command::Pick {
                kind: Menu::Mode,
                id,
            } => {
                let mode = MODES.iter().find(|(known, _)| *known == id);
                chat.mode = Some(mode.ok_or(Refusal::Gone)?.1);
            }
            Command::Pick {
                kind: Menu::Model,
                id,
            } => chat.model = Some(id),
            Command::Menu { .. } => unreachable!("menus were answered above"),
        }
        Ok(json!({"type": "done"}))
    }

    async fn say(&mut self, text: String) -> Result<(), Refusal> {
        let model = self.model().await?.ok_or(Refusal::NoModel)?;
        let chat = self.chat.as_mut().ok_or(Refusal::Offline)?;
        if let Place::Draft(workspace) = &chat.place {
            if !self.overview.borrow().has_workspace(workspace) {
                return Err(Refusal::Gone);
            }
            let session = self.phones.sessions.create(workspace).await?;
            chat.place = Place::Session(session);
            chat.sent = None;
        }
        if chat.events.is_none() {
            let session = chat.session().expect("a draft became a session above");
            chat.events = Some(Box::pin(self.phones.sessions.attach(session)?));
        }
        let mode = chat.mode();
        let session = chat.session().expect("a draft became a session above");
        self.phones
            .sessions
            .send(session, text, mode, model)
            .await?;
        Ok(())
    }

    /// The phone's choice, else the conversation's, else the first configured.
    async fn model(&mut self) -> Result<Option<pb::RequestedModel>> {
        let Some(chat) = &self.chat else {
            return Ok(None);
        };
        let of_session = chat
            .session()
            .and_then(|session| self.overview.borrow().model(session));
        let id = match (&chat.model, of_session) {
            (Some(id), Some(model)) if model.model_id == *id => return Ok(Some(model)),
            (Some(id), _) => Some(id.clone()),
            (None, Some(model)) => return Ok(Some(model)),
            (None, None) => {
                let models = self.phones.models.list().await?;
                models.into_iter().next().map(|model| model.id)
            }
        };
        Ok(id.map(|model_id| pb::RequestedModel {
            model_id,
            ..Default::default()
        }))
    }

    async fn relabel(&mut self) -> Result<()> {
        let label = match self.model().await? {
            Some(model) => {
                let models = self.phones.models.list().await?;
                let known = models.into_iter().find(|it| it.id == model.model_id);
                Some(known.map_or(model.model_id, |known| known.label))
            }
            None => None,
        };
        if let Some(chat) = &mut self.chat {
            chat.label = label;
        }
        Ok(())
    }

    fn reattach(&mut self) {
        let Some(chat) = &mut self.chat else { return };
        if chat.events.is_some() {
            return;
        }
        let Some(session) = chat.session() else {
            return;
        };
        // Not connected: the next tick tries again.
        if let Ok(events) = self.phones.sessions.attach(session) {
            chat.events = Some(Box::pin(events));
            chat.transcript = Transcript::default();
            chat.fresh = true;
        }
    }

    async fn hear(&mut self, event: Option<Result<pb::AgentHostSessionEvent>>) {
        let Some(chat) = &mut self.chat else { return };
        let event = match event {
            Some(Ok(event)) => event.event,
            Some(Err(error)) => {
                tracing::debug!(%error, "Cursor conversation lost");
                chat.events = None;
                return;
            }
            None => {
                chat.events = None;
                return;
            }
        };
        let Some(event) = event else { return };
        if let (true, Event::ConversationState(state), Some(session)) =
            (chat.fresh, &event, chat.session())
        {
            match self.phones.sessions.history(session, state).await {
                Ok(turns) => chat.transcript.restore(turns),
                Err(error) => tracing::warn!(%error, "cannot read the conversation so far"),
            }
        }
        chat.fresh = false;
        // Once Cursor reports a mode, that is the one the phone asked for.
        chat.transcript.apply(event);
    }

    async fn list(&mut self) -> Result<()> {
        let mut message = self.overview.borrow_and_update().view();
        message["type"] = "overview".into();
        self.relabel().await?;
        send(self.socket, message).await
    }

    /// Sends only the rows that changed, and nothing when nothing did.
    async fn publish(&mut self) -> Result<()> {
        let Some(chat) = &mut self.chat else {
            return Ok(());
        };
        let mode = chat.mode();
        let mut status = chat.transcript.status();
        status["type"] = "chat".into();
        status["session"] = json!(chat.session());
        status["draft"] = match &chat.place {
            Place::Draft(workspace) => json!(workspace),
            Place::Session(_) => Value::Null,
        };
        status["open"] = json!(!chat.fresh || chat.session().is_none());
        status["mode"] = json!(MODES.iter().find(|(_, it)| *it == mode).map(|(id, _)| id));
        status["model"] = json!(chat.label);
        let rows = chat.transcript.rows();
        let from = match &chat.sent {
            Some((sent, last)) => {
                let same = rows.iter().zip(sent).take_while(|(a, b)| a == b).count();
                if same == rows.len() && same == sent.len() && *last == status {
                    return Ok(());
                }
                same
            }
            None => 0,
        };
        let mut message = status.clone();
        message["from"] = json!(from);
        message["rows"] = json!(rows[from..]);
        let rows = rows.to_vec();
        chat.sent = Some((rows, status));
        send(self.socket, message).await
    }
}

/// The next event of the conversation on screen; waits forever when there is none.
async fn heard(chat: &mut Option<Chat>) -> Option<Result<pb::AgentHostSessionEvent>> {
    match chat.as_mut().and_then(|chat| chat.events.as_mut()) {
        Some(events) => events.next().await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
#[path = "web_tests.rs"]
mod tests;
