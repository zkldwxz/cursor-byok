//! Serves the Chrome extension its requests, and the desktop whether it is connected.

use axum::{
    extract::{DefaultBodyLimit, Query},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};

use crate::cursor::tools::browser_bridge::{BrowserReply, Next, BRIDGE};

/// What the extension asks for. Chrome sends its requests with `Sec-Fetch-Site:
/// none`, so they stay out of the same-origin guard; the token, which only the
/// native messaging host hands out, is what lets them in.
pub fn extension_router<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new()
        .route("/__byok-api__/api/browser-devtools/enter", post(enter))
        .route("/__byok-api__/api/browser-devtools/next", get(next))
        .route(
            "/__byok-api__/api/browser-devtools/reply",
            post(reply).layer(DefaultBodyLimit::max(8 * 1024 * 1024)),
        )
}

#[derive(Serialize)]
pub struct Status {
    connected: bool,
}

pub async fn status() -> Json<Status> {
    let connected = BRIDGE.connected().await;
    Json(Status { connected })
}

#[derive(Deserialize)]
struct Entry {
    browser: String,
    take: bool,
}

#[derive(Deserialize)]
struct Poll {
    browser: String,
}

async fn enter(headers: HeaderMap, Query(entry): Query<Entry>) -> Response {
    let Some(token) = token(&headers) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    answer(BRIDGE.enter(token, &entry.browser, entry.take).await)
}

async fn next(headers: HeaderMap, Query(poll): Query<Poll>) -> Response {
    let Some(token) = token(&headers) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    answer(BRIDGE.next(token, &poll.browser).await)
}

fn answer(next: Next) -> Response {
    match next {
        Next::Refused => StatusCode::UNAUTHORIZED.into_response(),
        Next::Busy => StatusCode::CONFLICT.into_response(),
        Next::Nothing => StatusCode::NO_CONTENT.into_response(),
        Next::Request(request) => Json(request).into_response(),
    }
}

async fn reply(headers: HeaderMap, Json(reply): Json<BrowserReply>) -> StatusCode {
    let Some(token) = token(&headers) else {
        return StatusCode::UNAUTHORIZED;
    };
    if BRIDGE.reply(token, reply).await {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
}

fn token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}
