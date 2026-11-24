//! Lets paired phones follow and steer Cursor's Agents Window through a temporary tunnel.
mod access;
mod markdown;
mod models;
pub(crate) mod presence;
mod relay;
mod sessions;
mod transcript;
mod tunnel;
mod web;

use std::{
    future::IntoFuture,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use serde::Serialize;
use tokio::{net::TcpListener, sync::Mutex};
use tokio_util::sync::CancellationToken;

use crate::{cursor::transport::MessageModes, plugin::PluginRegistry, store::Store, Error, Result};

use access::Access;
pub use access::PairedDevice;
use models::Models;
use relay::Relay;
use sessions::Sessions;
use tunnel::Tunnel;

/// Whether this server, rather than Cursor's own, answers Cursor's remote control.
#[derive(Clone, Default)]
pub struct RemoteSwitch(Arc<AtomicBool>);

impl RemoteSwitch {
    pub fn is_on(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    pub(crate) fn turn(&self, on: bool) {
        self.0.store(on, Ordering::Relaxed);
    }
}

#[derive(Clone)]
pub struct RemoteControl {
    inner: Arc<Inner>,
}

struct Inner {
    switch: RemoteSwitch,
    relay: Relay,
    sessions: Sessions,
    models: Models,
    cloudflared: fn() -> Option<PathBuf>,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    active: Option<Active>,
    /// Why remote control turned itself off.
    error: Option<String>,
}

struct Active {
    url: String,
    access: Arc<Access>,
    stop: CancellationToken,
}

#[derive(Clone, Debug, Serialize)]
pub struct RemoteStatus {
    pub enabled: bool,
    pub cloudflared: bool,
    /// Cursor's Agents Window is connected.
    pub cursor: bool,
    pub devices: Vec<PairedDevice>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PairingLink {
    pub link: String,
    pub qr_svg: String,
}

impl RemoteControl {
    pub fn new(store: Store, plugins: PluginRegistry, modes: MessageModes) -> Self {
        Self::build(
            Models::new(store, Some(plugins)),
            modes,
            tunnel::find_cloudflared,
        )
    }

    fn build(models: Models, modes: MessageModes, cloudflared: fn() -> Option<PathBuf>) -> Self {
        let relay = Relay::default();
        Self {
            inner: Arc::new(Inner {
                switch: RemoteSwitch::default(),
                sessions: Sessions::new(relay.clone(), modes),
                relay,
                models,
                cloudflared,
                state: Mutex::default(),
            }),
        }
    }

    pub fn switch(&self) -> RemoteSwitch {
        self.inner.switch.clone()
    }

    /// What Cursor calls instead of its own remote control service.
    pub fn presence_router(&self) -> axum::Router {
        presence::router(self.inner.relay.clone(), self.inner.switch.clone())
    }

    pub async fn status(&self) -> RemoteStatus {
        let cursor = self.inner.relay.connected().borrow().is_some();
        let state = self.inner.state.lock().await;
        RemoteStatus {
            enabled: state.active.is_some(),
            cloudflared: (self.inner.cloudflared)().is_some(),
            cursor,
            devices: state
                .active
                .as_ref()
                .map(|active| active.access.devices())
                .unwrap_or_default(),
            error: state.error.clone(),
        }
    }

    pub async fn enable(&self) -> Result<()> {
        let mut state = self.inner.state.lock().await;
        if state.active.is_some() {
            return Ok(());
        }
        state.error = None;
        let program = (self.inner.cloudflared)().ok_or_else(|| {
            Error::Config("cloudflared was not found; install it to open a tunnel".into())
        })?;
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let mut tunnel = Tunnel::start(&program, listener.local_addr()?).await?;
        let stop = CancellationToken::new();
        let access = Arc::new(Access::new(stop.clone()));
        let phones = web::Phones::new(
            access.clone(),
            self.inner.sessions.clone(),
            self.inner.models.clone(),
        );
        let server = axum::serve(listener, web::router(phones.clone()))
            .with_graceful_shutdown(stop.clone().cancelled_owned());
        self.inner.switch.turn(true);
        state.active = Some(Active {
            url: tunnel.url.clone(),
            access,
            stop: stop.clone(),
        });
        drop(state);
        let inner = self.inner.clone();
        tokio::spawn(async move {
            // Dropping the tunnel kills cloudflared.
            tokio::select! {
                () = stop.cancelled() => {}
                _ = server.into_future() => {}
                () = phones.follow() => {}
                () = tunnel.closed() => {
                    let mut state = inner.state.lock().await;
                    // Off and on again since: that tunnel is not this one.
                    if state.active.as_ref().is_some_and(|active| active.url == tunnel.url) {
                        state.active = None;
                        state.error = Some("the tunnel closed".into());
                        inner.leave();
                    }
                    stop.cancel();
                }
            }
        });
        Ok(())
    }

    pub async fn disable(&self) {
        let mut state = self.inner.state.lock().await;
        state.error = None;
        if let Some(active) = state.active.take() {
            active.stop.cancel();
            self.inner.leave();
        }
    }

    /// The code is in the fragment, which browsers send to no server.
    pub async fn pairing_link(&self) -> Result<PairingLink> {
        let state = self.inner.state.lock().await;
        let active = state
            .active
            .as_ref()
            .ok_or_else(|| Error::Config("remote control is off".into()))?;
        let link = format!("{}/#{}", active.url, active.access.new_pairing_code());
        let qr_svg = qrcode::QrCode::new(&link)
            .map_err(|error| Error::Config(format!("pairing link as a QR code: {error}")))?
            .render::<qrcode::render::svg::Color>()
            .min_dimensions(240, 240)
            .build();
        Ok(PairingLink { link, qr_svg })
    }

    pub async fn remove_device(&self, id: &str) -> Result<()> {
        let state = self.inner.state.lock().await;
        if state
            .active
            .as_ref()
            .is_some_and(|active| active.access.remove(id))
        {
            return Ok(());
        }
        Err(Error::Config(format!("paired device not found: {id}")))
    }
}

impl Inner {
    /// Cursor goes back to its own remote control.
    fn leave(&self) {
        self.switch.turn(false);
        self.relay.close();
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::{os::unix::fs::PermissionsExt, time::Duration};

    use super::*;

    fn fake_cloudflared(script: &str) -> Option<PathBuf> {
        let path = std::env::temp_dir().join(format!("byok-cloudflared-{}", uuid::Uuid::new_v4()));
        std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        Some(path)
    }

    fn lasting() -> Option<PathBuf> {
        fake_cloudflared("echo https://one-two.trycloudflare.com >&2; exec sleep 30")
    }

    fn short_lived() -> Option<PathBuf> {
        fake_cloudflared("echo https://one-two.trycloudflare.com >&2; sleep 0.3")
    }

    async fn remote(cloudflared: fn() -> Option<PathBuf>) -> RemoteControl {
        let directory = tempfile::tempdir().unwrap().keep();
        let url = format!("sqlite://{}", directory.join("state.db").display());
        let store = Store::connect(&url).await.unwrap();
        RemoteControl::build(
            Models::new(store, None),
            MessageModes::default(),
            cloudflared,
        )
    }

    #[tokio::test]
    async fn remote_control_is_off_until_turned_on_and_forgets_devices_when_turned_off() {
        let remote = remote(lasting).await;
        assert!(!remote.status().await.enabled);
        assert!(remote.pairing_link().await.is_err());
        remote.enable().await.unwrap();
        let pairing = remote.pairing_link().await.unwrap();
        assert!(pairing
            .link
            .starts_with("https://one-two.trycloudflare.com/#"));
        assert!(pairing.qr_svg.contains("<svg"));
        let code = pairing.link.split('#').nth(1).unwrap().to_string();
        {
            let state = remote.inner.state.lock().await;
            state.active.as_ref().unwrap().access.pair(&code, "phone");
        }
        assert_eq!(remote.status().await.devices.len(), 1);
        remote.disable().await;
        remote.enable().await.unwrap();
        assert!(remote.status().await.devices.is_empty());
        remote.disable().await;
    }

    #[tokio::test]
    async fn cursor_is_answered_and_kept_connected_only_while_remote_control_is_on() {
        let remote = remote(lasting).await;
        let ask = || async {
            let request = axum::http::Request::post(format!(
                "{}GetRemoteControlV2UserSettings",
                presence::PATH
            ))
            .body(axum::body::Body::empty())
            .unwrap();
            tower::ServiceExt::oneshot(remote.presence_router(), request)
                .await
                .unwrap()
                .status()
        };
        assert_eq!(ask().await, axum::http::StatusCode::SERVICE_UNAVAILABLE);
        remote.enable().await.unwrap();
        assert!(remote.switch().is_on());
        assert_eq!(ask().await, axum::http::StatusCode::OK);
        let _cursor = relay::testing::FakeCursor::connect(&remote.inner.relay);
        assert!(remote.status().await.cursor);
        remote.disable().await;
        assert!(!remote.switch().is_on());
        assert!(!remote.status().await.cursor);
    }

    #[tokio::test]
    async fn a_tunnel_that_closes_turns_remote_control_off_and_says_so() {
        let remote = remote(short_lived).await;
        remote.enable().await.unwrap();
        for _ in 0..40 {
            let status = remote.status().await;
            if !status.enabled {
                assert_eq!(status.error.as_deref(), Some("the tunnel closed"));
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("remote control stayed on without a tunnel");
    }

    #[tokio::test]
    async fn a_missing_cloudflared_is_named() {
        let error = remote(|| None).await.enable().await.unwrap_err();
        assert!(error.to_string().contains("cloudflared"), "{error}");
    }
}
