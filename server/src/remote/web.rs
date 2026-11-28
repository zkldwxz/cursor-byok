//! Serves paired phones and browsers: the page they load, pairing, and their live session.
use std::{pin::Pin, sync::Arc, time::Duration};

use axum::{
    extract::{State, WebSocketUpgrade},
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
    models::{set_context, set_effort, Model, Models},
    sessions::{Overview, Sessions, Verdict},
    ssh::Kept,
    transcript::{Row, Transcript},
    wire::Wire,
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
const CLIENT: [(&str, &str, &str); 22] = [
    ("/", HTML, include_str!("client/index.html")),
    ("/style.css", CSS, include_str!("client/style.css")),
    ("/icon.svg", SVG, include_str!("client/icon.svg")),
    ("/app.js", JS, include_str!("client/app.js")),
    ("/link.js", JS, include_str!("client/link.js")),
    ("/native.js", JS, include_str!("client/native.js")),
    ("/model.js", JS, include_str!("client/model.js")),
    ("/dom.js", JS, include_str!("client/dom.js")),
    ("/copy.js", JS, include_str!("client/copy.js")),
    ("/sidebar.js", JS, include_str!("client/sidebar.js")),
    ("/chat.js", JS, include_str!("client/chat.js")),
    ("/dock.js", JS, include_str!("client/dock.js")),
    (
        "/conversations.js",
        JS,
        include_str!("client/conversations.js"),
    ),
    ("/waiting.js", JS, include_str!("client/waiting.js")),
    ("/brands.js", JS, include_str!("client/brands.js")),
    ("/line.js", JS, include_str!("client/line.js")),
    ("/tune.js", JS, include_str!("client/tune.js")),
    ("/computers.js", JS, include_str!("client/computers.js")),
    ("/feel.js", JS, include_str!("client/feel.js")),
    ("/settings.js", JS, include_str!("client/settings.js")),
    ("/segments.js", JS, include_str!("client/segments.js")),
    ("/views.js", JS, include_str!("client/views.js")),
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
    /// The install id the phone or browser keeps, by which a re-pairing is
    /// one device rather than another entry.
    device: Option<String>,
}

async fn pair(
    State(phones): State<Phones>,
    headers: HeaderMap,
    Json(request): Json<PairRequest>,
) -> Response {
    if !same_origin(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    match phones
        .access
        .pair(&request.code, &request.name, request.device.as_deref())
        .await
    {
        Ok(Some(token)) => (
            [(header::CACHE_CONTROL, "no-store")],
            Json(json!({"token": token})),
        )
            .into_response(),
        Ok(None) => StatusCode::UNAUTHORIZED.into_response(),
        Err(error) => {
            tracing::warn!(%error, "cannot keep the paired phone");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
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
        .on_upgrade(move |socket| serve(Wire::Browser(Box::new(socket)), phones, None))
}

fn same_origin(headers: &HeaderMap) -> bool {
    let text = |name| headers.get(name).and_then(|value| value.to_str().ok());
    let origin = text(header::ORIGIN)
        .and_then(|origin| origin.parse::<Uri>().ok())
        .and_then(|origin| origin.authority().map(ToString::to_string));
    origin.is_some() && origin.as_deref() == text(header::HOST)
}

/// `place` is the link by which an app finds this computer next time.
pub(super) async fn serve(mut wire: Wire, phones: Phones, place: Option<String>) {
    let Ok(Some(session)) =
        tokio::time::timeout(AUTH_TIMEOUT, authenticate(&mut wire, &phones)).await
    else {
        let _ = wire.send(json!({"type": "unpaired"})).await;
        return;
    };
    let computer = json!({"type": "computer", "name": computer_name(), "link": place});
    if wire.send(computer).await.is_err() {
        return;
    }
    tokio::select! {
        () = session.cancelled() => {
            // Turned off, it may be on again: only a removed phone has to pair anew.
            if !phones.access.is_closed() {
                let _ = wire.send(json!({"type": "unpaired"})).await;
            }
        }
        result = Session::new(&mut wire, &phones).run() => {
            if let Err(error) = result {
                tracing::debug!(%error, "phone session ended");
            }
        }
    }
}

/// Tells this computer from a person's others.
fn computer_name() -> String {
    let name = gethostname::gethostname().to_string_lossy().into_owned();
    name.strip_suffix(".local").unwrap_or(&name).to_owned()
}

/// How a phone says who it is: as a message, so it never appears in a URL.
#[derive(Deserialize)]
#[serde(untagged)]
enum Hello {
    Paired { token: String },
    New(PairRequest),
}

async fn authenticate(wire: &mut Wire, phones: &Phones) -> Option<CancellationToken> {
    let token = match serde_json::from_str(&wire.receive().await?).ok()? {
        Hello::Paired { token } => token,
        Hello::New(request) => {
            let paired = phones
                .access
                .pair(&request.code, &request.name, request.device.as_deref())
                .await;
            let token = paired
                .inspect_err(|error| tracing::warn!(%error, "cannot keep the paired phone"))
                .ok()??;
            let paired = json!({"type": "paired", "token": token});
            wire.send(paired).await.ok()?;
            token
        }
    };
    phones.access.session(&token).await
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
    Context,
    Effort,
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
    /// Of an SSH host, which Cursor keeps from remote control.
    ReadOnly,
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
            Self::ReadOnly => "read_only",
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
    context: Option<String>,
    effort: Option<String>,
    /// The model as the phone shows it, and what is set of it.
    shown: Option<(Model, Value)>,
    /// What was last read of an SSH conversation.
    kept: Option<Kept>,
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
            context: None,
            effort: None,
            shown: None,
            kept: None,
            sent: None,
        }
    }

    fn session(&self) -> Option<&str> {
        match &self.place {
            Place::Session(id) => Some(id),
            Place::Draft(_) => None,
        }
    }

    /// Takes in what was added to an SSH conversation.
    async fn read(&mut self, sessions: &Sessions) {
        let Some(session) = self.session() else {
            return;
        };
        match sessions.kept(session, self.kept.as_ref()).await {
            Ok(Some(mut kept)) => {
                self.events = None;
                self.fresh = false;
                self.transcript = std::mem::take(&mut kept.transcript);
                self.shown = kept.model.clone().map(|model| (model, Value::Null));
                self.kept = Some(kept);
            }
            Ok(None) => {}
            Err(error) => tracing::warn!(%error, "cannot read the SSH conversation"),
        }
    }

    fn mode(&self) -> pb::AgentMode {
        self.mode
            .or(self.transcript.mode())
            .unwrap_or(pb::AgentMode::Agent)
    }
}

struct Session<'a> {
    wire: &'a mut Wire,
    phones: &'a Phones,
    overview: watch::Receiver<Overview>,
    chat: Option<Chat>,
}

impl<'a> Session<'a> {
    fn new(wire: &'a mut Wire, phones: &'a Phones) -> Self {
        Self {
            wire,
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
                message = self.wire.receive() => match message {
                    Some(text) => self.handle(&text).await?,
                    None => return Ok(()),
                },
                Ok(()) = self.overview.changed() => self.list().await?,
                event = heard(&mut self.chat) => self.hear(event).await,
                _ = publish.tick() => self.publish().await?,
                _ = reattach.tick() => self.reattach().await,
                _ = ping.tick() => self.wire.ping().await?,
            }
        }
    }

    async fn handle(&mut self, text: &str) -> Result<()> {
        match serde_json::from_str(text)? {
            Request::Overview => self.list().await,
            Request::Attach(id) => {
                self.chat = Some(Chat::new(Place::Session(id)));
                self.reattach().await;
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
                self.wire.send(reply).await?;
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
                // These come with the conversation, for its model.
                Menu::Context | Menu::Effort => return Err(Refusal::Gone),
            };
            return Ok(json!({"type": "menu", "items": items}));
        }
        if self.is_read_only() {
            return Err(Refusal::ReadOnly);
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
            } => {
                if chat.model.as_ref() != Some(&id) {
                    // Another model may not have what was set.
                    (chat.context, chat.effort) = (None, None);
                }
                chat.model = Some(id);
            }
            Command::Pick {
                kind: kind @ (Menu::Context | Menu::Effort),
                id,
            } => {
                let (model, _) = chat.shown.as_ref().ok_or(Refusal::NoModel)?;
                let (setting, set) = match kind {
                    Menu::Context => (&model.context, &mut chat.context),
                    _ => (&model.effort, &mut chat.effort),
                };
                if !setting.has(&id) {
                    return Err(Refusal::Gone);
                }
                *set = Some(id);
            }
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

    /// The model as the next message asks for it.
    async fn model(&mut self) -> Result<Option<pb::RequestedModel>> {
        let mut model = self.chosen().await?;
        if let (Some(model), Some(chat)) = (&mut model, &self.chat) {
            if let Some(context) = &chat.context {
                set_context(model, context);
            }
            if let Some(effort) = &chat.effort {
                set_effort(model, effort);
            }
        }
        Ok(model)
    }

    /// The phone's choice, else the conversation's, else the first configured.
    async fn chosen(&mut self) -> Result<Option<pb::RequestedModel>> {
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
        let shown = if self.is_read_only() {
            let kept = self.chat.as_ref().and_then(|chat| chat.kept.as_ref());
            let model = kept.and_then(|kept| kept.model.clone());
            model.map(|model| (model, Value::Null))
        } else if let Some(model) = self.model().await? {
            let known = self.phones.models.find(&model.model_id).await?;
            let known = known.unwrap_or_else(|| {
                let id = model.model_id.clone();
                Model::named(id.clone(), id.clone(), id)
            });
            let tuning = known.tuning(&model);
            Some((known, tuning))
        } else {
            None
        };
        if let Some(chat) = &mut self.chat {
            chat.shown = shown;
        }
        Ok(())
    }

    fn is_read_only(&self) -> bool {
        let session = self.chat.as_ref().and_then(Chat::session);
        session.is_some_and(|session| self.overview.borrow().is_ssh(session))
    }

    async fn reattach(&mut self) {
        let read_only = self.is_read_only();
        let Some(chat) = &mut self.chat else { return };
        if read_only {
            return chat.read(&self.phones.sessions).await;
        }
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
        self.wire.send(message).await
    }

    /// Sends only the rows that changed, and nothing when nothing did.
    async fn publish(&mut self) -> Result<()> {
        let read_only = self.is_read_only();
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
        status["model"] = json!(chat.shown.as_ref().map(|(model, _)| &model.label));
        status["modelName"] = json!(chat.shown.as_ref().map(|(model, _)| &model.name));
        status["tuning"] = json!(chat.shown.as_ref().map(|(_, tuning)| tuning));
        status["readOnly"] = json!(read_only);
        status["retry"] = json!(chat
            .session()
            .and_then(|session| self.phones.sessions.retry(session)));
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
        self.wire.send(message).await
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
