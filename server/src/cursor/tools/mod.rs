//! Exposes the extensible Cursor Tool system.
use std::{
    collections::{BTreeMap, HashSet},
    sync::Arc,
};

use tokio::sync::Mutex;

pub(crate) mod browser_bridge;
pub mod codec;
pub(crate) mod compat;
pub(crate) mod devtools;
pub(crate) mod edit;
pub(crate) mod registry;
pub mod runtime;
mod schedule;
pub mod semantic_search;
pub(crate) mod stream;
mod tool_call_dispatch;
pub(crate) mod tool_call_result;

use crate::{
    model::{CanonicalMessage, MessageContent, Role, ToolCall},
    provider::images::ImageGeneration,
    search::{WebCache, WebFetch, WebSearch},
    store::Store,
    Error, Result,
};

use self::schedule::{DeferredEdit, EditSchedule};
use self::tool_call_result::ToolResultSender;
pub use self::tool_call_result::{ToolCompletion, ToolResultReceiver};
use super::protocol::proto::agent::v1 as pb;
use runtime::{CursorToolRuntime, ExecContext};
use semantic_search::{SearchReply, SemanticPackage, Step};

#[derive(Clone)]
pub struct ToolDispatcher {
    runtime: CursorToolRuntime,
    results: ToolResultSender,
    search: WebSearch,
    fetch: WebFetch,
    /// Present where a store is: the channel's image model, when one is configured.
    images: Option<ImageGeneration>,
    /// The store the desktop language is read from, where one is configured.
    store: Option<Store>,
    semantic: SemanticPackage,
    edit_schedule: Arc<Mutex<EditSchedule>>,
}

pub struct DispatchedTool {
    pub messages: Vec<pb::AgentServerMessage>,
    pub completion: Option<ToolCompletion>,
}

impl DispatchedTool {
    fn pending() -> Self {
        Self {
            messages: Vec::new(),
            completion: None,
        }
    }
}

pub struct ToolBatchState<'a> {
    pub completed: &'a HashSet<String>,
    pub started: &'a HashSet<String>,
    pub response_text: &'a str,
    pub response_thinking: &'a str,
}

impl ToolDispatcher {
    pub fn new(runtime: CursorToolRuntime) -> Self {
        Self::observed(runtime).0
    }

    /// Also hands out the results of work that finishes in the background.
    pub fn observed(runtime: CursorToolRuntime) -> (Self, ToolResultReceiver) {
        let (results, receiver) = tool_call_result::tool_result_channel();
        let dispatcher = Self {
            runtime,
            results,
            search: WebSearch::built_in(),
            fetch: WebFetch::built_in(),
            images: None,
            store: None,
            semantic: SemanticPackage::built_in(),
            edit_schedule: Arc::new(Mutex::new(EditSchedule::default())),
        };
        (dispatcher, receiver)
    }

    pub fn with_semantic(mut self, semantic: SemanticPackage) -> Self {
        self.semantic = semantic;
        self
    }

    pub fn with_results(
        runtime: CursorToolRuntime,
        results: ToolResultSender,
        store: Store,
        web_cache: WebCache,
        semantic: SemanticPackage,
    ) -> Self {
        Self {
            runtime,
            results,
            images: Some(ImageGeneration::new(store.clone())),
            search: WebSearch::managed(store.clone()),
            fetch: WebFetch::managed(store.clone(), web_cache),
            store: Some(store),
            semantic,
            edit_schedule: Arc::new(Mutex::new(EditSchedule::default())),
        }
    }

    pub async fn start_batch(
        &self,
        calls: &[ToolCall],
        state: ToolBatchState<'_>,
        messages: &[CanonicalMessage],
        dynamic_mcp: &BTreeMap<String, pb::McpToolDefinition>,
        context: &ExecContext,
    ) -> Result<Vec<DispatchedTool>> {
        let first_tool_index = current_turn_step_count(messages)
            + usize::from(!state.response_thinking.is_empty())
            + usize::from(!state.response_text.is_empty())
            + 1;
        let mut dispatched = Vec::new();
        for (position, call) in calls.iter().enumerate() {
            if state.completed.contains(&call.call_id) {
                continue;
            }
            let message_index = first_tool_index + position;
            let publish_started = !state.started.contains(&call.call_id);
            if let Some(error) = &call.argument_error {
                dispatched.push(validation_failure(call, error.clone()));
                continue;
            }
            let edit_path = if dynamic_mcp.contains_key(&call.name) {
                None
            } else {
                match edit::execution_path(call) {
                    Ok(path) => path,
                    Err(error) => {
                        dispatched.push(recover_validation_failure(call, error)?);
                        continue;
                    }
                }
            };
            if let Some(path) = edit_path {
                let next = self.edit_schedule.lock().await.start_or_defer(
                    path,
                    DeferredEdit {
                        call: call.clone(),
                        message_index,
                        publish_started,
                        context: context.clone(),
                    },
                );
                let Some(next) = next else {
                    continue;
                };
                let started = self
                    .start(
                        &next.call,
                        next.message_index,
                        next.publish_started,
                        dynamic_mcp,
                        &next.context,
                    )
                    .await;
                dispatched.push(match started {
                    Ok(started) => started,
                    Err(error) => recover_validation_failure(&next.call, error)?,
                });
                continue;
            }
            let started = self
                .start(call, message_index, publish_started, dynamic_mcp, context)
                .await;
            dispatched.push(match started {
                Ok(started) => started,
                Err(error) => recover_validation_failure(call, error)?,
            });
        }
        Ok(dispatched)
    }

    pub(crate) async fn continue_after(&self, call_id: &str) -> Result<Option<DispatchedTool>> {
        let next = self.edit_schedule.lock().await.complete(call_id)?;
        let Some(next) = next else {
            return Ok(None);
        };
        match self
            .start(
                &next.call,
                next.message_index,
                next.publish_started,
                &BTreeMap::new(),
                &next.context,
            )
            .await
        {
            Ok(started) => Ok(Some(started)),
            Err(error) => recover_validation_failure(&next.call, error).map(Some),
        }
    }

    pub async fn interrupt_for_message(&self) -> Vec<u32> {
        self.edit_schedule.lock().await.clear();
        self.runtime.interrupt_for_message().await
    }

    /// Continues a search beside the conversation, which must stay responsive
    /// while the program downloads.
    pub fn continue_search(
        &self,
        reply: SearchReply,
        emit: impl FnOnce(pb::AgentServerMessage) + Send + 'static,
    ) {
        let (runtime, results, semantic) = (
            self.runtime.clone(),
            self.results.clone(),
            self.semantic.clone(),
        );
        tokio::spawn(async move {
            match semantic_search::advance(reply, &runtime, &semantic).await {
                Ok(Step::Send(message)) => emit(*message),
                Ok(Step::Complete(completion)) => results.send(*completion),
                Ok(Step::Interrupted) => {}
                Err(error) => results.send_error(error),
            }
        });
    }

    async fn start(
        &self,
        call: &ToolCall,
        message_index: usize,
        publish_started: bool,
        dynamic_mcp: &BTreeMap<String, pb::McpToolDefinition>,
        context: &ExecContext,
    ) -> Result<DispatchedTool> {
        let call = context.prepare_call(call)?;
        let mut messages = if publish_started {
            vec![codec::tool_started(&call, dynamic_mcp.get(&call.name))?]
        } else {
            Vec::new()
        };
        let started = tool_call_dispatch::start(
            &self.runtime,
            &self.results,
            &self.semantic,
            self.store.as_ref(),
            &call,
            message_index,
            dynamic_mcp,
            context,
        )
        .await?;
        messages.extend(started.messages);
        Ok(DispatchedTool {
            messages,
            completion: started.completion,
        })
    }

    pub async fn interaction_response(
        &self,
        response: &pb::InteractionResponse,
    ) -> Result<DispatchedTool> {
        if self.runtime.is_interrupted(response.id).await {
            return Ok(DispatchedTool::pending());
        }
        let pending = match self.runtime.take_interaction(response.id).await {
            Some(pending) => pending,
            None if self.runtime.completed_call(response.id).await.is_some() => {
                tracing::warn!(
                    id = response.id,
                    "ignoring duplicate terminal interaction response"
                );
                return Ok(DispatchedTool::pending());
            }
            None => {
                tracing::warn!(
                    id = response.id,
                    "ignoring response for unknown interaction"
                );
                return Ok(DispatchedTool::pending());
            }
        };
        let call = pending.call.clone();
        let continuation = match tool_call_dispatch::resume_interaction(
            &self.results,
            &self.search,
            &self.fetch,
            self.images.as_ref(),
            self.store.as_ref(),
            pending,
            response,
        )
        .await
        {
            Ok(continuation) => continuation,
            Err(Error::Protocol(message)) => {
                return Ok(validation_failure(&call, message));
            }
            Err(Error::Json(error)) => {
                return Ok(validation_failure(&call, error.to_string()));
            }
            Err(error) => return Err(error),
        };
        Ok(DispatchedTool {
            messages: continuation.messages,
            completion: continuation.completion,
        })
    }
}

fn validation_failure(call: &ToolCall, message: String) -> DispatchedTool {
    DispatchedTool {
        messages: Vec::new(),
        completion: Some(compat::failure_with_message(call, message)),
    }
}

fn recover_validation_failure(call: &ToolCall, error: Error) -> Result<DispatchedTool> {
    match error {
        Error::Protocol(message) => Ok(validation_failure(call, message)),
        Error::Json(error) => Ok(validation_failure(call, error.to_string())),
        error => Err(error),
    }
}

fn current_turn_step_count(messages: &[CanonicalMessage]) -> usize {
    let turn_start = messages
        .iter()
        .rposition(|message| message.role == Role::User)
        .map_or(0, |position| position + 1);
    messages[turn_start..]
        .iter()
        .map(|message| match &message.content {
            MessageContent::Assistant {
                text,
                thinking,
                tool_calls,
                ..
            } => {
                usize::from(!thinking.is_empty()) + usize::from(!text.is_empty()) + tool_calls.len()
            }
            _ => 0,
        })
        .sum()
}
