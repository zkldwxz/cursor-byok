//! What a phone can see of, and do with, the conversations in Cursor's Agents Window.
use std::{collections::BTreeMap, time::Duration};

use futures_util::{Stream, StreamExt};
use prost::Message;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::watch;
use uuid::Uuid;

use crate::{
    cursor::{
        conversation::UpstreamRetries,
        protocol::proto::agent::v1::{
            self as pb, agent_host_turn_awaiting_input::Question, interaction_query::Query,
            interaction_response::Result as Reply,
            respond_to_agent_host_interaction_request::Answer, watch_agent_host_sessions_event,
        },
        transport::MessageModes,
    },
    Error, Result,
};

use super::{
    relay::Relay,
    ssh::{Kept, SshChat, SshChats},
    transcript::file_name,
};

const WORKSPACES_INTERVAL: Duration = Duration::from_secs(10);
const RETRY: Duration = Duration::from_secs(2);
/// Older turns stay on the computer: a phone has no use for megabytes of them.
const HISTORY_TURNS: usize = 40;
/// Cursor refuses to be asked for more at once.
const MAX_BLOBS: usize = 64;

#[derive(Clone)]
pub(super) struct Sessions {
    relay: Relay,
    modes: MessageModes,
    ssh: SshChats,
    retries: UpstreamRetries,
}

/// What the agent was asked for is answered with one of these.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Verdict {
    Approve,
    Reject,
    Answers(Vec<QuestionAnswer>),
}

#[derive(Debug, Deserialize)]
pub(super) struct QuestionAnswer {
    question: String,
    #[serde(default)]
    options: Vec<String>,
    #[serde(default)]
    text: String,
}

/// Everything the Agents Window reports; `None` while it is not connected.
#[derive(Clone, Default, PartialEq)]
pub(super) struct Overview {
    open: Option<Open>,
}

#[derive(Clone, Default, PartialEq)]
struct Open {
    workspaces: Vec<String>,
    sessions: BTreeMap<String, pb::AgentHostSession>,
    /// Cursor keeps these from remote control; they can only be read.
    ssh: Vec<SshChat>,
}

/// A workspace as the phone lists it.
#[derive(Default)]
struct Listed<'a> {
    path: &'a str,
    host: Option<&'a str>,
    /// With when each was last changed.
    chats: Vec<(i64, Value)>,
}

impl Overview {
    pub fn has_workspace(&self, path: &str) -> bool {
        let open = self.open.as_ref();
        open.is_some_and(|open| open.workspaces.iter().any(|known| known == path))
    }

    pub fn is_ssh(&self, session: &str) -> bool {
        let open = self.open.as_ref();
        open.is_some_and(|open| open.ssh.iter().any(|chat| chat.id == session))
    }

    pub fn model(&self, session: &str) -> Option<pb::RequestedModel> {
        let session = self.open.as_ref()?.sessions.get(session)?;
        match &session.requested_model {
            Some(model) => Some(pb::RequestedModel {
                model_id: model.model_id.clone(),
                max_mode: model.max_mode,
                parameters: model.parameters.clone(),
                ..Default::default()
            }),
            None => session.model_id.clone().map(|model_id| pb::RequestedModel {
                model_id,
                ..Default::default()
            }),
        }
    }

    /// Workspaces with their conversations, the most recent first.
    pub fn view(&self) -> Value {
        let Some(open) = &self.open else {
            return json!({"reachable": false, "workspaces": []});
        };
        let mut listed: BTreeMap<String, Listed> = BTreeMap::new();
        for path in &open.workspaces {
            listed.entry(path.clone()).or_default().path = path;
        }
        // A subagent's session is part of the conversation that started it.
        for session in open.sessions.values().filter(|it| it.parent.is_none()) {
            let Some(path) = workspace_path(session.workspace.as_ref()) else {
                continue;
            };
            let workspace = listed.entry(path.into()).or_default();
            workspace.path = path;
            workspace.chats.push((
                updated_ms(session),
                json!({
                    "id": session.session_id,
                    "label": session.title.clone().unwrap_or_default(),
                    "state": state(session),
                    "updatedAt": updated_ms(session),
                }),
            ));
        }
        for chat in &open.ssh {
            let workspace = listed.entry(chat.workspace()).or_default();
            workspace.path = &chat.path;
            workspace.host = Some(&chat.host);
            workspace.chats.push((
                chat.updated_ms,
                json!({
                    "id": chat.id,
                    "label": chat.title,
                    "state": if chat.running { "running" } else { "idle" },
                    "updatedAt": chat.updated_ms,
                    "readOnly": true,
                }),
            ));
        }
        let mut listed = listed.into_iter().collect::<Vec<_>>();
        for (_, workspace) in &mut listed {
            workspace
                .chats
                .sort_by_key(|(updated, _)| std::cmp::Reverse(*updated));
        }
        listed.sort_by_key(|(_, workspace)| {
            std::cmp::Reverse(workspace.chats.first().map(|(updated, _)| *updated))
        });
        json!({
            "reachable": true,
            "workspaces": listed.into_iter().map(|(id, workspace)| json!({
                "id": id,
                "title": file_name(workspace.path),
                "host": workspace.host,
                "chats": workspace.chats.into_iter().map(|(_, chat)| chat).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        })
    }
}

/// Cursor leaves out a session's title and time when they did not change, as it
/// does once the session is opened; without them it would list as untitled and
/// as old as 1970. The rest is taken as sent: a missing running turn means none.
fn keep(sessions: &mut BTreeMap<String, pb::AgentHostSession>, mut session: pb::AgentHostSession) {
    if let Some(known) = sessions.get(&session.session_id) {
        session.title = session.title.or_else(|| known.title.clone());
        session.updated_at = session.updated_at.or(known.updated_at);
    }
    sessions.insert(session.session_id.clone(), session);
}

fn workspace_path(workspace: Option<&pb::AgentHostWorkspace>) -> Option<&str> {
    workspace?.workspace_paths.first().map(String::as_str)
}

fn updated_ms(session: &pb::AgentHostSession) -> i64 {
    session
        .updated_at
        .as_ref()
        .map(|time| time.seconds * 1_000 + i64::from(time.nanos) / 1_000_000)
        .unwrap_or(0)
}

fn state(session: &pb::AgentHostSession) -> &'static str {
    match session.status() {
        pb::AgentHostSessionStatus::Running => "running",
        pb::AgentHostSessionStatus::AwaitingInput => "waiting",
        _ => "idle",
    }
}

impl Sessions {
    pub fn new(relay: Relay, modes: MessageModes, ssh: SshChats, retries: UpstreamRetries) -> Self {
        Self {
            relay,
            modes,
            ssh,
            retries,
        }
    }

    /// Which time its model is being tried again; `None` while it answers.
    pub fn retry(&self, session_id: &str) -> Option<u32> {
        self.retries.attempt(session_id)
    }

    /// An SSH conversation, unless nothing was added since `last`.
    pub async fn kept(&self, session_id: &str, last: Option<&Kept>) -> Result<Option<Kept>> {
        let last = last.map(|kept| (kept.checkpoint, kept.running));
        self.ssh.read(session_id, last).await
    }

    /// Keeps `report` at what Cursor says, across reconnections, until dropped.
    pub async fn follow(&self, report: &watch::Sender<Overview>) {
        let mut connected = self.relay.connected();
        loop {
            let connection = *connected.borrow_and_update();
            if connection.is_some() {
                tokio::select! {
                    result = self.watch(report) => if let Err(error) = result {
                        tracing::debug!(%error, "Cursor session watch ended");
                    },
                    _ = connected.changed() => {}
                }
                tokio::time::sleep(RETRY).await;
            } else {
                report.send_replace(Overview::default());
                if connected.changed().await.is_err() {
                    return;
                }
            }
        }
    }

    async fn watch(&self, report: &watch::Sender<Overview>) -> Result<()> {
        use watch_agent_host_sessions_event::Event;
        let request = pb::WatchAgentHostSessionsRequest {
            all_workspaces: Some(true),
        };
        let mut events = Box::pin(
            self.relay
                .stream::<_, pb::WatchAgentHostSessionsEvent>("WatchSessions", &request)?,
        );
        let mut refresh = tokio::time::interval(WORKSPACES_INTERVAL);
        let mut open = Open::default();
        loop {
            tokio::select! {
                event = events.next() => {
                    let Some(event) = event else { return Ok(()) };
                    match event?.event {
                        Some(Event::Snapshot(snapshot)) => {
                            open.sessions = snapshot
                                .sessions
                                .into_iter()
                                .map(|session| (session.session_id.clone(), session))
                                .collect();
                        }
                        Some(
                            Event::Added(pb::AgentHostSessionAdded { session })
                            | Event::Updated(pb::AgentHostSessionUpdated { session }),
                        ) => {
                            if let Some(session) = session {
                                keep(&mut open.sessions, session);
                            }
                        }
                        Some(Event::Deleted(deleted)) => {
                            open.sessions.remove(&deleted.session_id);
                        }
                        // A keepalive.
                        None => {}
                    }
                }
                _ = refresh.tick() => {
                    open.workspaces = self.workspaces().await?;
                    match self.ssh.list().await {
                        Ok(chats) => open.ssh = chats,
                        // Those of this computer are still worth showing.
                        Err(error) => tracing::warn!(%error, "cannot list the SSH conversations"),
                    }
                }
            }
            report.send_if_modified(|last| {
                let now = Some(open.clone());
                let changed = last.open != now;
                last.open = now;
                changed
            });
        }
    }

    async fn workspaces(&self) -> Result<Vec<String>> {
        let response: pb::ListAgentHostWorkspacesResponse = self
            .relay
            .call("ListWorkspaces", &pb::ListAgentHostWorkspacesRequest {})
            .await?;
        Ok(response
            .workspaces
            .iter()
            .filter_map(|workspace| workspace_path(Some(workspace)).map(str::to_owned))
            .collect())
    }

    pub async fn create(&self, workspace: &str) -> Result<String> {
        let session_id = Uuid::new_v4().to_string();
        let request = pb::CreateAgentHostSessionRequest {
            session_id: session_id.clone(),
            session_options: Some(pb::AgentHostSessionOptions {
                workspace_paths: vec![workspace.into()],
                ..Default::default()
            }),
            ..Default::default()
        };
        self.relay
            .call::<_, pb::CreateAgentHostSessionResponse>("CreateSession", &request)
            .await?;
        Ok(session_id)
    }

    pub fn attach(
        &self,
        session_id: &str,
    ) -> Result<impl Stream<Item = Result<pb::AgentHostSessionEvent>> + Send + 'static> {
        self.relay.stream(
            "AttachSession",
            &pb::AttachAgentHostSessionRequest {
                session_id: session_id.into(),
                last_event_id: None,
            },
        )
    }

    /// The turns a saved state refers to, the latest ones only.
    pub async fn history(
        &self,
        session_id: &str,
        state: &pb::ConversationStateStructure,
    ) -> Result<Vec<pb::AgentConversationTurn>> {
        use pb::conversation_turn_structure::Turn;
        let first = state.turns.len().saturating_sub(HISTORY_TURNS);
        let ids = &state.turns[first..];
        let blobs = self.blobs(session_id, ids.to_vec()).await?;
        let mut structures = Vec::with_capacity(ids.len());
        for id in ids {
            let Some(data) = blobs.get(id) else { continue };
            let turn = pb::ConversationTurnStructure::decode(data.as_slice())?.turn;
            if let Some(Turn::AgentConversationTurn(turn)) = turn {
                structures.push(turn);
            }
        }
        let parts = structures
            .iter()
            .flat_map(|turn| std::iter::once(&turn.user_message).chain(&turn.steps))
            .filter(|id| !id.is_empty())
            .cloned()
            .collect();
        let blobs = self.blobs(session_id, parts).await?;
        structures
            .into_iter()
            .map(|turn| {
                let user = blobs.get(&turn.user_message);
                let steps = turn.steps.iter().filter_map(|id| blobs.get(id));
                Ok(pb::AgentConversationTurn {
                    user_message: user
                        .map(|data| pb::UserMessage::decode(data.as_slice()))
                        .transpose()?,
                    steps: steps
                        .map(|data| Ok(pb::ConversationStep::decode(data.as_slice())?))
                        .collect::<Result<_>>()?,
                    request_id: turn.request_id,
                })
            })
            .collect()
    }

    async fn blobs(
        &self,
        session_id: &str,
        wanted: Vec<Vec<u8>>,
    ) -> Result<BTreeMap<Vec<u8>, Vec<u8>>> {
        let mut found = BTreeMap::new();
        for wanted in wanted.chunks(MAX_BLOBS) {
            found.extend(self.few_blobs(session_id, wanted.to_vec()).await?);
        }
        Ok(found)
    }

    async fn few_blobs(
        &self,
        session_id: &str,
        mut wanted: Vec<Vec<u8>>,
    ) -> Result<BTreeMap<Vec<u8>, Vec<u8>>> {
        let mut found = BTreeMap::new();
        while !wanted.is_empty() {
            let response: pb::GetAgentHostSessionBlobsResponse = self
                .relay
                .call(
                    "GetSessionBlobs",
                    &pb::GetAgentHostSessionBlobsRequest {
                        session_id: session_id.into(),
                        blob_ids: wanted,
                        max_response_bytes: None,
                    },
                )
                .await?;
            // Nothing new: what is left does not fit any answer.
            if response.blobs.is_empty() {
                break;
            }
            found.extend(response.blobs.into_iter().map(|it| (it.blob_id, it.data)));
            wanted = response.remaining_blob_ids;
        }
        Ok(found)
    }

    pub async fn send(
        &self,
        session_id: &str,
        text: String,
        mode: pb::AgentMode,
        model: pb::RequestedModel,
    ) -> Result<()> {
        let message_id = Uuid::new_v4().to_string();
        self.modes.choose(message_id.clone(), mode);
        let request = pb::SendAgentHostMessageRequest {
            session_id: session_id.into(),
            text,
            model_id: Some(model.model_id.clone()),
            requested_model: Some(model),
            message_id: Some(message_id),
            ..Default::default()
        };
        self.relay
            .call::<_, pb::SendAgentHostMessageResponse>("SendMessage", &request)
            .await
            .map(drop)
    }

    pub async fn stop(&self, session_id: &str, turn_id: &str) -> Result<()> {
        let request = pb::InterruptAgentHostTurnRequest {
            session_id: session_id.into(),
            turn_id: turn_id.into(),
            reason: None,
        };
        self.relay
            .call::<_, pb::InterruptAgentHostTurnResponse>("InterruptTurn", &request)
            .await
            .map(drop)
    }

    /// Whether the answer counted: not when it was already given elsewhere.
    pub async fn respond(
        &self,
        session_id: &str,
        asked: &pb::AgentHostTurnAwaitingInput,
        verdict: Verdict,
    ) -> Result<bool> {
        let request = pb::RespondToAgentHostInteractionRequest {
            session_id: session_id.into(),
            interaction_id: asked.interaction_id.clone(),
            answer: Some(answer(asked, verdict)?),
        };
        let response: pb::RespondToAgentHostInteractionResponse =
            self.relay.call("RespondToInteraction", &request).await?;
        Ok(response.status() == pb::AgentHostInteractionResponseStatus::Accepted)
    }
}

fn answer(asked: &pb::AgentHostTurnAwaitingInput, verdict: Verdict) -> Result<Answer> {
    const DECLINED: &str = "The user declined.";
    let approved = matches!(verdict, Verdict::Approve);
    let query = match &asked.question {
        Some(Question::ExecApproval(_)) if !matches!(verdict, Verdict::Answers(_)) => {
            return Ok(Answer::ExecApproval(pb::AgentHostApprovalResult {
                approved,
                reason: (!approved).then(|| DECLINED.into()),
                ..Default::default()
            }));
        }
        Some(Question::InteractionQuery(query)) => query,
        _ => return Err(unanswerable()),
    };
    /// These answers differ only in their type.
    macro_rules! verdict {
        ($reply:ident, $module:ident) => {
            Reply::$reply(pb::$reply {
                result: Some(if approved {
                    pb::$module::Result::Approved(Default::default())
                } else {
                    pb::$module::Result::Rejected(pb::$module::Rejected {
                        reason: DECLINED.into(),
                    })
                }),
            })
        };
    }
    let result = match (&query.query, verdict) {
        (Some(Query::AskQuestionInteractionQuery(_)), verdict) => {
            use pb::ask_question_result::Result as Outcome;
            let result = match verdict {
                Verdict::Answers(answers) => Outcome::Success(pb::AskQuestionSuccess {
                    answers: answers
                        .into_iter()
                        .map(|answer| pb::ask_question_success::Answer {
                            question_id: answer.question,
                            selected_option_ids: answer.options,
                            freeform_text: answer.text,
                        })
                        .collect(),
                }),
                _ => Outcome::Rejected(pb::AskQuestionRejected {
                    reason: DECLINED.into(),
                }),
            };
            Reply::AskQuestionInteractionResponse(pb::AskQuestionInteractionResponse {
                result: Some(pb::AskQuestionResult {
                    result: Some(result),
                }),
            })
        }
        (_, Verdict::Answers(_)) => return Err(unanswerable()),
        (Some(Query::WebSearchRequestQuery(_)), _) => {
            verdict!(WebSearchRequestResponse, web_search_request_response)
        }
        (Some(Query::WebFetchRequestQuery(_)), _) => {
            verdict!(WebFetchRequestResponse, web_fetch_request_response)
        }
        (Some(Query::SwitchModeRequestQuery(_)), _) => {
            verdict!(SwitchModeRequestResponse, switch_mode_request_response)
        }
        (Some(Query::CreatePlanRequestQuery(_)), _) => {
            use pb::create_plan_result::Result as Outcome;
            Reply::CreatePlanRequestResponse(pb::CreatePlanRequestResponse {
                result: Some(pb::CreatePlanResult {
                    result: Some(if approved {
                        Outcome::Success(pb::CreatePlanSuccess {})
                    } else {
                        Outcome::Error(pb::CreatePlanError {
                            error: DECLINED.into(),
                        })
                    }),
                    ..Default::default()
                }),
            })
        }
        _ => return Err(unanswerable()),
    };
    Ok(Answer::InteractionResponse(pb::InteractionResponse {
        id: query.id,
        result: Some(result),
    }))
}

fn unanswerable() -> Error {
    Error::Protocol("this question can be answered only on the computer".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(
        title: Option<&str>,
        seconds: Option<i64>,
        running: Option<&str>,
    ) -> pb::AgentHostSession {
        pb::AgentHostSession {
            session_id: "s".into(),
            title: title.map(Into::into),
            updated_at: seconds.map(|seconds| prost_types::Timestamp { seconds, nanos: 0 }),
            running_turn_id: running.map(Into::into),
            ..Default::default()
        }
    }

    #[test]
    fn an_update_without_title_or_time_keeps_the_known_ones_and_takes_the_rest() {
        let mut sessions = BTreeMap::new();
        keep(
            &mut sessions,
            session(Some("General chat"), Some(100), Some("turn")),
        );
        keep(&mut sessions, session(None, None, None));
        let kept = &sessions["s"];
        assert_eq!(kept.title.as_deref(), Some("General chat"));
        assert_eq!(updated_ms(kept), 100_000);
        assert_eq!(
            kept.running_turn_id, None,
            "a turn that ended is not kept running"
        );

        keep(&mut sessions, session(Some("Renamed"), Some(200), None));
        assert_eq!(sessions["s"].title.as_deref(), Some("Renamed"));
        assert_eq!(updated_ms(&sessions["s"]), 200_000);
    }
}
