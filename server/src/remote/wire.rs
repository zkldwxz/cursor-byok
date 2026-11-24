//! Carries a phone's messages, whether it came by browser or by app.
use axum::extract::ws::{Message, WebSocket};
use serde_json::Value;

use crate::{Error, Result};

pub(super) enum Wire {
    Browser(Box<WebSocket>),
    App(remote_link::Link),
}

impl Wire {
    /// `None` once the phone is gone. Safe to cancel.
    pub async fn receive(&mut self) -> Option<String> {
        match self {
            Self::Browser(socket) => loop {
                match socket.recv().await?.ok()? {
                    Message::Text(text) => return Some(text.to_string()),
                    Message::Close(_) => return None,
                    _ => {}
                }
            },
            Self::App(link) => link.receive().await,
        }
    }

    pub async fn send(&mut self, message: Value) -> Result<()> {
        let message = message.to_string();
        match self {
            Self::Browser(socket) => socket.send(Message::text(message)).await.map_err(lost),
            Self::App(link) => link.send(&message).await.map_err(lost),
        }
    }

    /// An app's connection keeps itself up.
    pub async fn ping(&mut self) -> Result<()> {
        match self {
            Self::Browser(socket) => socket
                .send(Message::Ping(Default::default()))
                .await
                .map_err(lost),
            Self::App(_) => Ok(()),
        }
    }
}

fn lost(error: impl std::fmt::Display) -> Error {
    Error::Provider(format!("phone connection: {error}"))
}
