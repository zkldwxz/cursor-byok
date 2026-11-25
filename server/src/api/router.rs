//! Builds the top-level server router.

use crate::{
    cursor::transport::TransportRegistry, network::NetworkClients, remote::RemoteControl, Result,
};

pub fn router(
    registry: TransportRegistry,
    clients: NetworkClients,
    remote: &RemoteControl,
) -> Result<axum::Router> {
    super::cursor::router(registry, clients, remote)
}
