//! Implements Cursor analytics endpoints and event handling.
use axum::{
    body::{Body, Bytes},
    extract::Extension,
    http::{header, HeaderValue, Request, Response, StatusCode},
};
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine,
};
use bytes::{BufMut, BytesMut};
use prost::Message;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::{api::cursor::proxy, Error, Result};

pub const BOOTSTRAP_STATSIG_PATH: &str = "/aiserver.v1.AnalyticsService/BootstrapStatsig";
const AGENT_RETRIES_GATE: &str = "nal_agent_retries";
const LOCAL_RULE: &str = "local_enabled";
// Cursor ramps this gate to a client-side agent loop whose BidiAppend wire
// dialect the local server cannot decode; pinning it off keeps runs on the
// server-side loop this server implements.
const LOCAL_LOOP_GATE: &str = "agent_host_local_loop";
// Cursor discards background subagent completions while this gate is off, which
// leaves the parent conversation waiting on a subagent that already finished.
const LONG_RUNNING_JOBS_GATE: &str = "long_running_jobs";
// Cursor serves remote control from its Agents Window only with both on.
const REMOTE_GATES: [&str; 2] = ["cursor_agent_host", "remote_agent_host_glass"];

#[derive(Clone, PartialEq, Message)]
struct BootstrapStatsigResponse {
    #[prost(string, tag = "1")]
    config: String,
    #[prost(uint64, tag = "2")]
    generated_at_ms: u64,
}

pub async fn bootstrap_statsig(
    Extension(upstream): Extension<proxy::CursorProxy>,
    request: Request<Body>,
) -> Result<Response<Body>> {
    let user = account_id(request.headers());
    match proxy::forward_buffered(&upstream, request).await {
        Ok(response) if response.status.is_success() => match patch_upstream(response) {
            Ok(response) => Ok(response),
            Err(error) => {
                tracing::warn!(%error, "Cursor Statsig bootstrap was invalid; using local bootstrap");
                local_response(&user)
            }
        },
        Ok(response) => {
            tracing::warn!(status = %response.status, "Cursor Statsig bootstrap was rejected; using local bootstrap");
            local_response(&user)
        }
        Err(error) => {
            tracing::warn!(%error, "Cursor Statsig bootstrap was unavailable; using local bootstrap");
            local_response(&user)
        }
    }
}

fn patch_upstream(response: proxy::BufferedResponse) -> Result<Response<Body>> {
    let (framed, payload) = unary_payload(&response.body)?;
    let mut message = BootstrapStatsigResponse::decode(payload)?;
    let mut config = serde_json::from_str::<Value>(&message.config)?;
    patch_gates(&mut config)?;
    message.config = serde_json::to_string(&config)?;
    Ok(response.with_body(encode_unary(&message, framed)))
}

fn patch_gates(config: &mut Value) -> Result<()> {
    for name in REMOTE_GATES {
        force_gate(config, name, true)?;
    }
    force_gate(config, AGENT_RETRIES_GATE, true)?;
    force_gate(config, LONG_RUNNING_JOBS_GATE, true)?;
    force_gate(config, LOCAL_LOOP_GATE, false)
}

fn force_gate(config: &mut Value, name: &str, value: bool) -> Result<()> {
    let gate_key = statsig_key(config, name);
    let root = config
        .as_object_mut()
        .ok_or_else(|| Error::Protocol("Statsig bootstrap config must be an object".into()))?;
    let gates = root
        .entry("feature_gates")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| Error::Protocol("Statsig feature_gates must be an object".into()))?;
    gates.insert(gate_key.clone(), gate(&gate_key, value));
    Ok(())
}

/// Cursor looks gates up under the account it is signed in to, and finds none
/// in a bootstrap made out to anyone else.
fn account_id(headers: &header::HeaderMap) -> String {
    let subject = || {
        let token = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
        let payload = token.strip_prefix("Bearer ")?.split('.').nth(1)?;
        let claims: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).ok()?).ok()?;
        Some(claims.get("sub")?.as_str()?.to_owned())
    };
    subject().unwrap_or_else(|| "local_ultra".into())
}

fn local_response(user: &str) -> Result<Response<Body>> {
    let generated_at_ms = chrono::Utc::now().timestamp_millis() as u64;
    let mut config = json!({
        "feature_gates": {},
        "dynamic_configs": {},
        "layer_configs": {},
        "user": {
            "userID": user,
            "customIDs": { "localUserID": "local_ultra" }
        },
        "has_updates": true,
        "hash_used": "none",
        "sdkParams": {
            "stableID": "local_ultra",
            "disableDiagnosticsLogging": true
        },
        "time": generated_at_ms
    });
    patch_gates(&mut config)?;
    let message = BootstrapStatsigResponse {
        config: serde_json::to_string(&config)?,
        generated_at_ms,
    };
    let body = message.encode_to_vec();
    let mut response = Response::new(Body::from(body.clone()));
    *response.status_mut() = StatusCode::OK;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/proto"),
    );
    response.headers_mut().insert(
        header::CONTENT_LENGTH,
        body.len()
            .to_string()
            .parse()
            .expect("body length is a valid header value"),
    );
    Ok(response)
}

fn statsig_key(config: &Value, name: &str) -> String {
    match config.get("hash_used").and_then(Value::as_str) {
        Some("djb2") => djb2(name),
        Some("sha256") => STANDARD.encode(Sha256::digest(name.as_bytes())),
        _ => name.to_owned(),
    }
}

fn djb2(value: &str) -> String {
    value
        .encode_utf16()
        .fold(0_u32, |hash, character| {
            hash.wrapping_mul(31).wrapping_add(u32::from(character))
        })
        .to_string()
}

fn gate(name: &str, value: bool) -> Value {
    json!({
        "name": name,
        "value": value,
        "rule_id": LOCAL_RULE,
        "ruleID": LOCAL_RULE,
        "group_name": LOCAL_RULE,
        "groupName": LOCAL_RULE,
        "secondary_exposures": [],
        "secondaryExposures": [],
        "undelegated_secondary_exposures": [],
        "undelegatedSecondaryExposures": [],
        "is_device_based": false,
        "isDeviceBased": false,
        "id_type": "userID",
        "idType": "userID"
    })
}

fn unary_payload(body: &Bytes) -> Result<(bool, &[u8])> {
    if body.len() < 5 {
        return Ok((false, body));
    }
    let flags = body[0];
    let length = u32::from_be_bytes([body[1], body[2], body[3], body[4]]) as usize;
    if length != body.len() - 5 {
        return Ok((false, body));
    }
    if flags != 0 {
        return Err(Error::Protocol(format!(
            "cannot patch compressed or terminal Statsig frame: flags={flags}"
        )));
    }
    Ok((true, &body[5..]))
}

fn encode_unary(message: &impl Message, framed: bool) -> Bytes {
    let payload = message.encode_to_vec();
    if !framed {
        return Bytes::from(payload);
    }
    let mut output = BytesMut::with_capacity(5 + payload.len());
    output.put_u8(0);
    output.put_u32(payload.len() as u32);
    output.extend_from_slice(&payload);
    output.freeze()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statsig_bootstrap_keeps_the_agent_loop_server_side() {
        // Real bootstraps hash gate names; this user is enrolled in the 50%
        // agent_host_local_loop rollout that switches the client to a wire
        // dialect the local server cannot serve.
        let mut config = json!({
            "hash_used": "djb2",
            "feature_gates": {
                "1967931573": {
                    "name": "1967931573",
                    "value": true,
                    "rule_id": "74FdOxogOo75aIlcPjrAA8:50.00:4"
                }
            }
        });
        patch_gates(&mut config).expect("patch djb2 config");
        let gate = &config["feature_gates"][&djb2(LOCAL_LOOP_GATE)];
        assert_eq!(gate["value"], json!(false), "local loop must stay off");
    }

    #[test]
    fn statsig_bootstrap_disable_survives_other_hash_modes() {
        for hash_used in ["sha256", "none"] {
            let mut config = json!({
                "hash_used": hash_used,
                "feature_gates": {},
            });
            patch_gates(&mut config).expect("patch config");
            let key = statsig_key(&config, LOCAL_LOOP_GATE);
            assert_eq!(
                config["feature_gates"][&key]["value"],
                json!(false),
                "gate must be explicitly false under {hash_used}"
            );
        }
    }

    // With this gate off Cursor drops every background subagent completion, so
    // the parent conversation never learns that its subagent finished.
    #[test]
    fn statsig_bootstrap_keeps_background_completions_flowing() {
        for hash_used in ["djb2", "sha256", "none"] {
            let mut config = json!({ "hash_used": hash_used, "feature_gates": {} });
            let key = statsig_key(&config, LONG_RUNNING_JOBS_GATE);
            config["feature_gates"][&key] = gate(&key, false);
            patch_gates(&mut config).expect("patch config");
            assert_eq!(
                config["feature_gates"][&key]["value"],
                json!(true),
                "{hash_used}"
            );
        }
    }

    #[test]
    fn statsig_bootstrap_still_enables_agent_retries() {
        let mut config = json!({
            "hash_used": "djb2",
            "feature_gates": {},
        });
        patch_gates(&mut config).expect("patch config");
        let key = statsig_key(&config, AGENT_RETRIES_GATE);
        assert_eq!(config["feature_gates"][&key]["value"], json!(true));
    }

    #[tokio::test]
    async fn local_bootstrap_belongs_to_the_account_cursor_is_signed_in_to() {
        let mut headers = header::HeaderMap::new();
        let token = crate::local_app::local_cursor_authorization();
        headers.insert(header::AUTHORIZATION, token.parse().unwrap());
        let response = local_response(&account_id(&headers)).unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let message = BootstrapStatsigResponse::decode(body).unwrap();
        let config: Value = serde_json::from_str(&message.config).unwrap();
        assert_eq!(config["user"]["userID"], "cursor-local-user");
    }

    #[test]
    /// Cursor reads these as its window opens; turning remote control on later
    /// must not wait for the next window.
    fn statsig_bootstrap_always_opens_remote_control() {
        let mut config = json!({ "hash_used": "none", "feature_gates": {} });
        patch_gates(&mut config).expect("patch config");
        for name in REMOTE_GATES {
            assert_eq!(
                config["feature_gates"][name]["value"],
                json!(true),
                "{name}"
            );
        }
    }
}
