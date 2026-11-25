//! Answers Cursor's remote control service while remote control is on.
use axum::{
    body::Body,
    extract::State,
    http::{header, HeaderName, HeaderValue, Request, Response, StatusCode},
    response::IntoResponse,
    routing::post,
    Router,
};
use prost::Message;

use super::{relay::Relay, RemoteSwitch};

pub(crate) const PATH: &str = "/aiserver.v1.RemoteAgentHostPresenceService/";
const ENABLED: i32 = 1;

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

#[derive(Clone)]
struct Presence {
    relay: Relay,
    switch: RemoteSwitch,
}

pub(super) fn router(relay: Relay, switch: RemoteSwitch) -> Router {
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
        router = router.route(
            &format!("{PATH}{method}"),
            post(move |State(presence): State<Presence>| async move { presence.unary(answer) }),
        );
    }
    router
        .route(&format!("{PATH}ConnectTarget"), post(connect))
        .with_state(Presence { relay, switch })
}

impl Presence {
    fn unary(&self, answer: axum::body::Bytes) -> Response<Body> {
        if !self.switch.is_on() {
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        let content_type = HeaderValue::from_static("application/proto");
        ([(header::CONTENT_TYPE, content_type)], answer).into_response()
    }
}

async fn connect(State(presence): State<Presence>, request: Request<Body>) -> Response<Body> {
    if !presence.switch.is_on() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let outgoing = presence
        .relay
        .accept(request.into_body().into_data_stream());
    (
        [
            (header::CONTENT_TYPE, "application/connect+proto"),
            (HeaderName::from_static("connect-accept-encoding"), "gzip"),
        ],
        Body::from_stream(outgoing),
    )
        .into_response()
}
