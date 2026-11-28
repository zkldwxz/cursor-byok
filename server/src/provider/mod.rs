//! Defines the provider interface and exports provider implementations.
mod anthropic;
mod attempt;
mod event;
pub(crate) mod images;
pub(crate) mod magpie;
mod normalize;
mod openai_chat;
mod openai_responses;
mod recorder;
mod request_template;
mod router;

use std::pin::Pin;

use futures_util::Stream;
use tokio_util::sync::CancellationToken;

use crate::{model::ModelInvocation, Result};

pub use anthropic::AnthropicProvider;
pub use event::*;
pub use openai_chat::OpenAiChatProvider;
pub use openai_responses::OpenAiResponsesProvider;
pub use recorder::CallRecorder;
pub(crate) use router::custom_headers;

pub(crate) fn native_usage(
    kind: crate::model::ProviderType,
    value: &serde_json::Value,
) -> crate::model::Usage {
    match kind {
        crate::model::ProviderType::OpenAiChat => openai_chat::openai_usage(value),
        crate::model::ProviderType::OpenAiResponses => openai_responses::responses_usage(value),
        crate::model::ProviderType::Anthropic => anthropic::anthropic_usage(value),
        crate::model::ProviderType::Plugin => {
            unreachable!("plugin calls have no native HTTP protocol")
        }
    }
}
pub use router::{build as build_provider, ProviderRouter};

pub type ProviderStream = Pin<Box<dyn Stream<Item = Result<ModelEvent>> + Send>>;

pub trait Provider: Send + Sync {
    fn stream(
        &self,
        invocation: ModelInvocation,
        cancellation: CancellationToken,
    ) -> ProviderStream;
}

fn map_sse_error(
    label: &str,
    error: eventsource_stream::EventStreamError<crate::Error>,
) -> crate::Error {
    match error {
        eventsource_stream::EventStreamError::Transport(error) => error,
        eventsource_stream::EventStreamError::Utf8(error) => {
            crate::Error::Provider(format!("{label} SSE UTF-8 error: {error}"))
        }
        eventsource_stream::EventStreamError::Parser(error) => {
            crate::Error::Provider(format!("{label} SSE parse error: {error}"))
        }
    }
}

fn provider_event_error(label: &str, value: &serde_json::Value) -> Option<crate::Error> {
    let kind = value.get("type").and_then(serde_json::Value::as_str);
    let direct_error = value.get("error").filter(|error| !error.is_null());
    if !matches!(kind, Some("error" | "response.failed")) && direct_error.is_none() {
        return None;
    }

    let message = value
        .get("message")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            value
                .pointer("/error/message")
                .and_then(serde_json::Value::as_str)
        })
        .or_else(|| {
            value
                .pointer("/response/error/message")
                .and_then(serde_json::Value::as_str)
        })
        .or_else(|| direct_error.and_then(serde_json::Value::as_str))
        .or_else(|| {
            value
                .pointer("/response/error")
                .and_then(serde_json::Value::as_str)
        })
        .unwrap_or("provider returned an error event without a message");

    Some(crate::Error::Provider(format!("{label} error: {message}")))
}

fn merge_extra_params(body: &mut serde_json::Value, extra: &serde_json::Value) -> Result<()> {
    let extra = extra
        .as_object()
        .ok_or_else(|| crate::Error::Config("model extra params must be an object".into()))?;
    let body = body
        .as_object_mut()
        .ok_or_else(|| crate::Error::Provider("provider request body must be an object".into()))?;
    for (name, value) in extra {
        if matches!(
            name.as_str(),
            "model"
                | "stream"
                | "messages"
                | "input"
                | "tools"
                | "system"
                | "instructions"
                | "prompt_cache_key"
        ) {
            return Err(crate::Error::Config(format!(
                "model extra params cannot replace {name}"
            )));
        }
        body.insert(name.clone(), value.clone());
    }
    Ok(())
}

fn apply_body_allowlist(
    body: &mut serde_json::Value,
    allowed: Option<&std::collections::HashSet<String>>,
) -> Result<()> {
    let Some(allowed) = allowed else {
        return Ok(());
    };
    body.as_object_mut()
        .ok_or_else(|| crate::Error::Provider("provider request body must be an object".into()))?
        .retain(|name, _| allowed.contains(name));
    Ok(())
}

/// Pins a conversation to the machine that holds its prefix. The key is the
/// conversation's own, because a shared one mixes unrelated histories upstream.
fn apply_openai_prompt_cache_key(
    body: &mut serde_json::Value,
    model_id: &str,
    conversation_id: &str,
) -> Result<()> {
    if !model_id.to_ascii_lowercase().contains("gpt") {
        return Ok(());
    }
    body.as_object_mut()
        .ok_or_else(|| crate::Error::Provider("provider request body must be an object".into()))?
        .insert(
            "prompt_cache_key".into(),
            serde_json::Value::String(conversation_id.into()),
        );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sends one request through a provider and returns the body upstream saw.
    async fn sent_body(
        kind: crate::config::ProviderKind,
        model_id: &str,
        conversation_id: &str,
    ) -> serde_json::Value {
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let app = axum::Router::new().fallback(
            move |axum::Json(body): axum::Json<serde_json::Value>| async move {
                tx.send(body).await.unwrap();
                ([("content-type", "text/event-stream")], "data: [DONE]\n\n")
            },
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let config = crate::config::ProviderConfig {
            kind: kind.clone(),
            request_url: format!("http://{}/", listener.local_addr().unwrap()),
            api_key: "test".into(),
            custom_headers: Default::default(),
            max_output_tokens: None,
            request_timeout: std::time::Duration::from_secs(2),
            allowed_body_fields: None,
        };
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let provider: Box<dyn Provider> = match kind {
            crate::config::ProviderKind::OpenAiChat => {
                Box::new(OpenAiChatProvider::new(client, config))
            }
            crate::config::ProviderKind::OpenAiResponses => {
                Box::new(OpenAiResponsesProvider::new(client, config))
            }
            crate::config::ProviderKind::Anthropic => unreachable!("no OpenAI cache key"),
        };
        let mut stream = provider.stream(
            ModelInvocation {
                call_id: "call".into(),
                run_id: "run".into(),
                conversation_id: conversation_id.into(),
                provider_call_index: 0,
                request: crate::model::ModelRequest {
                    prompt: crate::model::PromptSpec {
                        instructions: "system".into(),
                        tools: Vec::new(),
                    },
                    model: crate::model::ModelSpec::new(model_id),
                    history: Vec::new(),
                },
            },
            CancellationToken::new(),
        );
        let _ = futures_util::StreamExt::next(&mut stream).await;
        let body = rx.recv().await.unwrap();
        server.abort();
        body
    }

    /// OpenAI routes a request to the machine that cached its key. One key for
    /// every conversation spreads them over machines that hold other histories.
    #[tokio::test]
    async fn each_conversation_keeps_its_own_openai_cache_key() {
        for kind in [
            crate::config::ProviderKind::OpenAiChat,
            crate::config::ProviderKind::OpenAiResponses,
        ] {
            let first = sent_body(kind.clone(), "gpt-6.2-sol", "conversation-a").await;
            let next = sent_body(kind.clone(), "gpt-6.2-sol", "conversation-a").await;
            let other = sent_body(kind.clone(), "gpt-6.2-sol", "conversation-b").await;
            assert_eq!(first["prompt_cache_key"], "conversation-a");
            assert_eq!(next["prompt_cache_key"], first["prompt_cache_key"]);
            assert_eq!(other["prompt_cache_key"], "conversation-b");

            // Other vendors reject the field, so it stays off their requests.
            let vendor = sent_body(kind, "deepseek-flash", "conversation-a").await;
            assert!(vendor.get("prompt_cache_key").is_none());
        }
    }

    #[test]
    fn sse_transport_errors_are_not_relabelled_as_parse_errors() {
        let error = map_sse_error(
            "test provider",
            eventsource_stream::EventStreamError::Transport(crate::Error::Provider(
                "connection closed".into(),
            )),
        );

        let crate::Error::Provider(message) = error else {
            panic!("transport error category must be preserved");
        };
        assert_eq!(message, "connection closed");
    }

    #[test]
    fn provider_error_events_extract_flat_and_nested_messages() {
        assert_provider_error(
            "OpenAI Responses",
            serde_json::json!({
                "type": "error",
                "message": "Internal error during token generation"
            }),
            "OpenAI Responses error: Internal error during token generation",
        );
        assert_provider_error(
            "OpenAI Chat",
            serde_json::json!({
                "error": {"message": "quota exceeded", "type": "server_error"}
            }),
            "OpenAI Chat error: quota exceeded",
        );
        assert_provider_error(
            "Anthropic",
            serde_json::json!({
                "type": "error",
                "error": {"type": "overloaded_error", "message": "Overloaded"}
            }),
            "Anthropic error: Overloaded",
        );
        assert_provider_error(
            "OpenAI Responses",
            serde_json::json!({
                "type": "response.failed",
                "response": {"error": {"message": "generation failed"}}
            }),
            "OpenAI Responses error: generation failed",
        );
    }

    #[test]
    fn successful_provider_events_are_not_errors() {
        assert!(provider_event_error(
            "OpenAI Responses",
            &serde_json::json!({"type": "response.completed", "error": null})
        )
        .is_none());
    }

    fn assert_provider_error(label: &str, value: serde_json::Value, expected: &str) {
        let Some(crate::Error::Provider(message)) = provider_event_error(label, &value) else {
            panic!("expected provider error");
        };
        assert_eq!(message, expected);
    }
}
