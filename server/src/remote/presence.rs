//! Answers Cursor's remote control service in place of Cursor's own.
use axum::{
    body::Body,
    extract::State,
    http::{header, HeaderName, HeaderValue, Request, Response},
    response::IntoResponse,
    routing::post,
    Router,
};
use prost::Message;

use super::relay::Relay;

pub(crate) const PATH: &str = "/aiserver.v1.RemoteAgentHostPresenceService/";
/// `RemoteControlV2RegistrationState`; 1 is `NOT_ENROLLED`.
const ENABLED: i32 = 2;

#[derive(Clone, PartialEq, Message)]
struct UserSettings {
    #[prost(bool, optional, tag = "2")]
    automatic_registration_enabled: Option<bool>,
}

#[derive(Clone, PartialEq, Message)]
struct Policy {
    #[prost(bool, optional, tag = "1")]
    enabled: Option<bool>,
    #[prost(bool, optional, tag = "2")]
    automatic_registration_enabled: Option<bool>,
    #[prost(bool, optional, tag = "3")]
    available: Option<bool>,
    #[prost(bool, optional, tag = "4")]
    automatic_registration_released: Option<bool>,
}

#[derive(Clone, PartialEq, Message)]
struct UserSettingsResponse {
    #[prost(message, optional, tag = "1")]
    settings: Option<UserSettings>,
    #[prost(message, optional, tag = "2")]
    policy: Option<Policy>,
}

#[derive(Clone, PartialEq, Message)]
struct RegistrationState {
    #[prost(int32, tag = "1")]
    state: i32,
    #[prost(bool, optional, tag = "3")]
    has_enrollment_history: Option<bool>,
}

#[derive(Clone, PartialEq, Message)]
struct Enrollment {
    #[prost(uint64, tag = "1")]
    generation: u64,
}

pub(super) fn router(relay: Relay) -> Router {
    let settings = UserSettingsResponse {
        settings: Some(UserSettings {
            automatic_registration_enabled: Some(true),
        }),
        policy: Some(Policy {
            enabled: Some(true),
            automatic_registration_enabled: Some(true),
            available: Some(true),
            automatic_registration_released: Some(true),
        }),
    }
    .encode_to_vec();
    // No history is what makes Cursor register this computer by itself.
    let registration = RegistrationState {
        state: ENABLED,
        has_enrollment_history: Some(false),
    }
    .encode_to_vec();
    let enrollment = Enrollment { generation: 1 }.encode_to_vec();
    let mut router = Router::new();
    for (method, answer) in [
        ("GetRemoteControlV2UserSettings", settings.clone()),
        ("UpdateRemoteControlV2UserSettings", settings),
        ("GetSharedTargetRegistrationState", registration),
        ("EnableSharedTarget", enrollment),
        ("RevokeSharedTarget", Vec::new()),
        ("ListPendingControllerApprovals", Vec::new()),
        ("ListSharedTargets", Vec::new()),
        ("ListControllers", Vec::new()),
    ] {
        let answer = axum::body::Bytes::from(answer);
        router = router.route(&format!("{PATH}{method}"), post(move || unary(answer)));
    }
    router
        .route(&format!("{PATH}ConnectTarget"), post(connect))
        .with_state(relay)
}

async fn unary(answer: axum::body::Bytes) -> Response<Body> {
    let content_type = HeaderValue::from_static("application/proto");
    ([(header::CONTENT_TYPE, content_type)], answer).into_response()
}

async fn connect(State(relay): State<Relay>, request: Request<Body>) -> Response<Body> {
    let outgoing = relay.accept(request.into_body().into_data_stream());
    (
        [
            (header::CONTENT_TYPE, "application/connect+proto"),
            (HeaderName::from_static("connect-accept-encoding"), "gzip"),
        ],
        Body::from_stream(outgoing),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;

    /// A Cursor that registered before reads this before it connects, and
    /// stays away for half an hour when told it is not registered.
    #[tokio::test]
    async fn a_restarted_cursor_is_told_this_computer_is_still_registered() {
        let request = Request::post(format!("{PATH}GetSharedTargetRegistrationState"))
            .body(Body::empty())
            .unwrap();
        let response = tower::ServiceExt::oneshot(router(Relay::default()), request)
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        // Field 1 is Cursor's own number for ENABLED, field 3 an explicit false.
        assert_eq!(body.as_ref(), [0x08, 2, 0x18, 0]);
    }
}
