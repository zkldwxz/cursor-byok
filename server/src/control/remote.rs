//! Turns remote control on and off and manages the phones paired with it.
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;

use crate::{
    remote::{PairingLink, RemoteStatus},
    store::RelaySettings,
    Result,
};

use super::ControlService;

#[derive(Deserialize)]
pub struct EnableInput {
    enabled: bool,
}

pub async fn status(State(service): State<ControlService>) -> Result<Json<RemoteStatus>> {
    Ok(Json(service.remote().status().await?))
}

pub async fn set_relay(
    State(service): State<ControlService>,
    Json(relay): Json<RelaySettings>,
) -> Result<Json<RemoteStatus>> {
    service.remote().set_relay(relay).await?;
    Ok(Json(service.remote().status().await?))
}

pub async fn set_enabled(
    State(service): State<ControlService>,
    Json(input): Json<EnableInput>,
) -> Result<Json<RemoteStatus>> {
    if input.enabled {
        service.remote().enable().await?;
    } else {
        service.remote().disable().await?;
    }
    Ok(Json(service.remote().status().await?))
}

pub async fn pair(State(service): State<ControlService>) -> Result<Json<PairingLink>> {
    Ok(Json(service.remote().pairing_link().await?))
}

pub async fn remove_device(
    State(service): State<ControlService>,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    service.remote().remove_device(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}
