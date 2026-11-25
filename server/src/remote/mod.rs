//! Lets paired phones follow and steer Cursor's Agents Window, from the app or from a browser.
mod access;
mod app;
mod kept;
mod markdown;
mod models;
pub(crate) mod presence;
mod relay;
mod sessions;
mod ssh;
mod transcript;
mod tunnel;
mod web;
mod wire;

use std::{
    future::IntoFuture,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use remote_link::{Host, Ticket};
use serde::Serialize;
use tokio::{net::TcpListener, sync::Mutex};
use tokio_util::sync::CancellationToken;

use crate::{
    cursor::{conversation::UpstreamRetries, transport::MessageModes},
    local_app::account,
    plugin::PluginRegistry,
    store::{RelaySettings, Store},
    Error, Result,
};

use access::Access;
pub use access::PairedDevice;
use models::Models;
use relay::Relay;
use sessions::Sessions;
use ssh::SshChats;
use tunnel::Tunnel;

/// Long enough for phones to hear that they are no longer paired.
const FAREWELL: Duration = Duration::from_secs(1);

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
    store: Store,
    cloudflared: fn() -> Option<PathBuf>,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    active: Option<Active>,
    /// Why browsers have no way in.
    error: Option<String>,
}

struct Active {
    host: Arc<Host>,
    /// Where browsers find it, while a tunnel is open.
    url: Option<String>,
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
    pub relay: RelaySettings,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PairingLink {
    pub app: Qr,
    /// While a tunnel is open. Whichever is used first spends the code of both.
    pub web: Option<Qr>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Qr {
    pub link: String,
    pub svg: String,
}

impl Qr {
    fn new(link: String) -> Result<Self> {
        let svg = qrcode::QrCode::with_error_correction_level(&link, qrcode::EcLevel::L)
            .map_err(|error| Error::Config(format!("pairing link as a QR code: {error}")))?
            .render::<qrcode::render::svg::Color>()
            .min_dimensions(240, 240)
            .build();
        Ok(Self { link, svg })
    }
}

impl RemoteControl {
    pub fn new(
        store: Store,
        plugins: PluginRegistry,
        modes: MessageModes,
        retries: UpstreamRetries,
    ) -> Self {
        let ssh = SshChats::new(store.clone(), account::state_db_path().ok());
        let relay = Relay::default();
        let sessions = Sessions::new(relay.clone(), modes, ssh, retries);
        Self::build(
            store,
            Some(plugins),
            relay,
            sessions,
            tunnel::find_cloudflared,
        )
    }

    fn build(
        store: Store,
        plugins: Option<PluginRegistry>,
        relay: Relay,
        sessions: Sessions,
        cloudflared: fn() -> Option<PathBuf>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                switch: RemoteSwitch::default(),
                sessions,
                relay,
                models: Models::new(store.clone(), plugins),
                store,
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

    pub async fn status(&self) -> Result<RemoteStatus> {
        let relay = self.inner.store.relay_settings().await?;
        let cursor = self.inner.relay.connected().borrow().is_some();
        let state = self.inner.state.lock().await;
        Ok(RemoteStatus {
            enabled: state.active.is_some(),
            cloudflared: (self.inner.cloudflared)().is_some(),
            cursor,
            devices: state
                .active
                .as_ref()
                .map(|active| active.access.devices())
                .unwrap_or_default(),
            relay,
            error: state.error.clone(),
        })
    }

    /// Counts from the next time remote control is turned on.
    pub async fn set_relay(&self, settings: RelaySettings) -> Result<()> {
        let settings = settings.trimmed();
        if let Some(relay) = relay_of(&settings) {
            relay.check().map_err(remote_error)?;
        }
        self.inner.store.set_relay_settings(&settings).await
    }

    pub async fn enable(&self) -> Result<()> {
        let mut state = self.inner.state.lock().await;
        if state.active.is_some() {
            return Ok(());
        }
        state.error = None;
        let relay = relay_of(&self.inner.store.relay_settings().await?);
        let host = Arc::new(Host::bind(relay).await.map_err(remote_error)?);
        let stop = CancellationToken::new();
        let access = Arc::new(Access::new(stop.clone()));
        let phones = web::Phones::new(
            access.clone(),
            self.inner.sessions.clone(),
            self.inner.models.clone(),
        );
        self.inner.switch.turn(true);
        state.active = Some(Active {
            host: host.clone(),
            url: None,
            access,
            stop: stop.clone(),
        });
        drop(state);
        tokio::spawn({
            let (phones, stop) = (phones.clone(), stop.clone());
            async move {
                tokio::select! {
                    () = stop.cancelled() => tokio::time::sleep(FAREWELL).await,
                    () = app::listen(&host, &phones) => {}
                    () = phones.follow() => {}
                }
                host.close().await;
            }
        });
        if let Some(program) = (self.inner.cloudflared)() {
            let inner = self.inner.clone();
            tokio::spawn(async move {
                let Err(error) = inner.admit_browsers(&program, phones, &stop).await else {
                    return;
                };
                tracing::warn!(%error, "browsers cannot reach remote control");
                let mut state = inner.state.lock().await;
                // Off since, and perhaps on again: that is not this time's tunnel.
                if stop.is_cancelled() {
                    return;
                }
                state.error = Some(error.to_string());
                if let Some(active) = &mut state.active {
                    active.url = None;
                }
            });
        }
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

    pub async fn pairing_link(&self) -> Result<PairingLink> {
        let state = self.inner.state.lock().await;
        let active = state
            .active
            .as_ref()
            .ok_or_else(|| Error::Config("remote control is off".into()))?;
        let code = active.access.new_pairing_code();
        // Browsers send the fragment to no server.
        let web = active.url.as_ref().map(|url| format!("{url}/#{code}"));
        let ticket: Ticket = active.host.ticket(code);
        Ok(PairingLink {
            app: Qr::new(ticket.link())?,
            web: web.map(Qr::new).transpose()?,
        })
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
    /// Until told to stop, or until the tunnel is lost.
    async fn admit_browsers(
        &self,
        program: &std::path::Path,
        phones: web::Phones,
        stop: &CancellationToken,
    ) -> Result<()> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let starting = Tunnel::start(program, listener.local_addr()?);
        // Dropping the tunnel kills cloudflared.
        let mut tunnel = tokio::select! {
            () = stop.cancelled() => return Ok(()),
            tunnel = starting => tunnel?,
        };
        {
            let mut state = self.state.lock().await;
            if stop.is_cancelled() {
                return Ok(());
            }
            if let Some(active) = &mut state.active {
                active.url = Some(tunnel.url.clone());
            }
        }
        let server = axum::serve(listener, web::router(phones))
            .with_graceful_shutdown(stop.clone().cancelled_owned());
        tokio::select! {
            () = stop.cancelled() => Ok(()),
            result = server.into_future() => Ok(result?),
            () = tunnel.closed() => Err(Error::Config("the tunnel closed".into())),
        }
    }

    /// Cursor goes back to its own remote control.
    fn leave(&self) {
        self.switch.turn(false);
        self.relay.close();
    }
}

fn relay_of(settings: &RelaySettings) -> Option<remote_link::Relay> {
    (!settings.address.is_empty()).then(|| remote_link::Relay {
        address: settings.address.clone(),
        token: (!settings.token.is_empty()).then(|| settings.token.clone()),
    })
}

fn remote_error(error: remote_link::Error) -> Error {
    Error::Config(error.to_string())
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;

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
        let relay = Relay::default();
        let sessions = Sessions::new(
            relay.clone(),
            MessageModes::default(),
            SshChats::new(store.clone(), None),
            UpstreamRetries::default(),
        );
        RemoteControl::build(store, None, relay, sessions, cloudflared)
    }

    /// The browser's link, once cloudflared has opened its tunnel.
    async fn web_link(remote: &RemoteControl) -> Qr {
        for _ in 0..40 {
            if let Some(web) = remote.pairing_link().await.unwrap().web {
                return web;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("the tunnel did not open");
    }

    #[tokio::test]
    async fn remote_control_is_off_until_turned_on_and_forgets_devices_when_turned_off() {
        let remote = remote(lasting).await;
        assert!(!remote.status().await.unwrap().enabled);
        assert!(remote.pairing_link().await.is_err());
        remote.enable().await.unwrap();
        let web = web_link(&remote).await;
        assert!(web.link.starts_with("https://one-two.trycloudflare.com/#"));
        assert!(web.svg.contains("<svg"));
        let code = web.link.split('#').nth(1).unwrap().to_string();
        {
            let state = remote.inner.state.lock().await;
            state.active.as_ref().unwrap().access.pair(&code, "phone");
        }
        assert_eq!(remote.status().await.unwrap().devices.len(), 1);
        remote.disable().await;
        remote.enable().await.unwrap();
        assert!(remote.status().await.unwrap().devices.is_empty());
        remote.disable().await;
    }

    #[tokio::test]
    async fn the_app_needs_no_tunnel() {
        let remote = remote(|| None).await;
        remote.enable().await.unwrap();
        let pairing = remote.pairing_link().await.unwrap();
        assert!(pairing.web.is_none());
        assert!(!remote.status().await.unwrap().cloudflared);
        let ticket = remote_link::Ticket::parse(&pairing.app.link).unwrap();
        let phone = remote_link::Phone::bind(None).await.unwrap();
        let mut link = phone.dial(&ticket).await.unwrap();
        let hello = serde_json::json!({"code": ticket.code, "name": "phone"});
        link.send(&hello.to_string()).await.unwrap();
        assert!(link.receive().await.unwrap().contains("paired"));
        remote.disable().await;
        let ended = tokio::time::timeout(Duration::from_secs(10), async {
            while link.receive().await.is_some() {}
        });
        assert!(ended.await.is_ok());
    }

    #[tokio::test]
    async fn a_relay_is_kept_and_given_to_the_phone() {
        let remote = remote(|| None).await;
        let relay = RelaySettings {
            address: " http://127.0.0.1:9 ".into(),
            token: " secret ".into(),
        };
        remote.set_relay(relay).await.unwrap();
        let kept = remote.status().await.unwrap().relay;
        assert_eq!(
            (kept.address.as_str(), kept.token.as_str()),
            ("http://127.0.0.1:9", "secret")
        );
        assert!(remote
            .set_relay(RelaySettings {
                address: "nowhere".into(),
                token: String::new()
            })
            .await
            .is_err());
        assert_eq!(remote.status().await.unwrap().relay, kept);
        remote.set_relay(RelaySettings::default()).await.unwrap();
        assert_eq!(
            remote.status().await.unwrap().relay,
            RelaySettings::default()
        );
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
        assert!(remote.status().await.unwrap().cursor);
        remote.disable().await;
        assert!(!remote.switch().is_on());
        assert!(!remote.status().await.unwrap().cursor);
    }

    #[tokio::test]
    async fn a_tunnel_that_closes_ends_the_browser_link_and_leaves_the_app() {
        let remote = remote(short_lived).await;
        remote.enable().await.unwrap();
        web_link(&remote).await;
        for _ in 0..40 {
            let status = remote.status().await.unwrap();
            if status.error.is_some() {
                assert!(status.error.unwrap().contains("the tunnel closed"));
                assert!(status.enabled);
                assert!(remote.pairing_link().await.unwrap().web.is_none());
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("the closed tunnel went unnoticed");
    }

    #[tokio::test]
    async fn a_tunnel_of_an_earlier_time_does_not_touch_this_one() {
        let remote = remote(short_lived).await;
        remote.enable().await.unwrap();
        web_link(&remote).await;
        remote.disable().await;
        remote.enable().await.unwrap();
        assert_eq!(remote.status().await.unwrap().error, None);
        web_link(&remote).await;
    }
}
