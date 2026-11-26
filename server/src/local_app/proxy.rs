//! Configures the local application proxy.
use std::{net::SocketAddr, sync::Arc};

use hudsucker::{
    certificate_authority::RcgenAuthority,
    hyper::{Request, Uri},
    rustls::crypto::aws_lc_rs,
    Body, HttpContext, HttpHandler, Proxy, RequestOrResponse,
};
use tokio::{net::TcpListener, sync::oneshot, task::JoinHandle};

use parking_lot::RwLock;

use crate::{
    api::cursor::proxy::UPSTREAM_URL_HEADER, cursor::services::tab::is_tab_path, remote::presence,
    store::TabMode, Error, Result,
};

use super::ca::LoadedCa;

#[derive(Default)]
pub struct ProxyRuntime {
    url: Option<String>,
    port: Option<u16>,
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

impl ProxyRuntime {
    pub fn running(&self) -> bool {
        self.task.as_ref().is_some_and(|task| !task.is_finished())
    }
    pub fn url(&self) -> Option<String> {
        self.running().then(|| self.url.clone()).flatten()
    }
    pub fn port(&self) -> Option<u16> {
        if self.running() {
            self.port
        } else {
            None
        }
    }

    pub async fn start(
        &mut self,
        backend: SocketAddr,
        ca: LoadedCa,
        requested_port: u16,
        tab_mode: Arc<RwLock<TabMode>>,
    ) -> Result<(String, u16)> {
        if let Some(url) = self.url() {
            return Ok((url, self.port.unwrap_or_default()));
        }
        let listener = bind_proxy_listener(requested_port).await?;
        let address = listener.local_addr()?;
        let (stop, done) = oneshot::channel();
        let authority = RcgenAuthority::new(ca.issuer, 1_000, aws_lc_rs::default_provider());
        let proxy = Proxy::builder()
            .with_listener(listener)
            .with_ca(authority)
            .with_rustls_connector(aws_lc_rs::default_provider())
            .with_http_handler(CursorRelay { backend, tab_mode })
            .with_graceful_shutdown(async move {
                let _ = done.await;
            })
            .build()
            .map_err(|error| Error::Store(format!("build Cursor proxy: {error}")))?;
        self.stop = Some(stop);
        self.url = Some(format!("http://{address}"));
        self.port = Some(address.port());
        self.task = Some(tokio::spawn(async move {
            if let Err(error) = proxy.start().await {
                tracing::error!(%error, "Cursor proxy stopped unexpectedly");
            }
        }));
        Ok((self.url.clone().unwrap(), address.port()))
    }

    pub async fn stop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), task).await;
        }
        self.url = None;
        self.port = None;
    }
}

async fn bind_proxy_listener(requested_port: u16) -> Result<TcpListener> {
    let requested = SocketAddr::from(([127, 0, 0, 1], requested_port));
    match TcpListener::bind(requested).await {
        Ok(listener) => Ok(listener),
        Err(error) if requested_port != 0 => {
            tracing::warn!(%requested, %error, "configured proxy port unavailable; selecting a random port");
            Ok(TcpListener::bind("127.0.0.1:0").await?)
        }
        Err(error) => Err(error.into()),
    }
}

#[derive(Clone)]
struct CursorRelay {
    backend: SocketAddr,
    tab_mode: Arc<RwLock<TabMode>>,
}

impl HttpHandler for CursorRelay {
    async fn handle_request(
        &mut self,
        _ctx: &HttpContext,
        mut request: Request<Body>,
    ) -> RequestOrResponse {
        let original = request.uri().clone();
        let locally_routed = should_route_locally(original.path(), *self.tab_mode.read());
        if is_cursor_host(original.host().unwrap_or_default()) && locally_routed {
            if let Ok(value) = original.to_string().parse() {
                request.headers_mut().insert(UPSTREAM_URL_HEADER, value);
            }
            let path = original
                .path_and_query()
                .map(|value| value.as_str())
                .unwrap_or("/");
            if let Ok(uri) = format!("http://{}{}", self.backend, path).parse::<Uri>() {
                *request.uri_mut() = uri;
            }
        }
        request.into()
    }

    async fn should_intercept_connect(
        &mut self,
        _ctx: &HttpContext,
        request: &Request<Body>,
    ) -> bool {
        request
            .uri()
            .authority()
            .is_some_and(|authority| is_cursor_host(authority.host()))
    }

    async fn should_intercept_tls(
        &mut self,
        _ctx: &HttpContext,
        hello: hudsucker::rustls::server::ClientHello<'_>,
    ) -> bool {
        hello.server_name().is_some_and(is_cursor_host)
    }
}

pub fn is_cursor_host(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    matches!(host.as_str(), "api2.cursor.sh" | "api3.cursor.sh") || host.ends_with(".cursor.sh")
}

fn is_local_path(path: &str) -> bool {
    matches!(
        path,
        "/agent.v1.AgentService/RunSSE"
            | "/aiserver.v1.BidiService/BidiAppend"
            | "/aiserver.v1.AiService/AvailableDocs"
            | "/aiserver.v1.DashboardService/GetEffectiveUserPlugins"
            | "/aiserver.v1.DashboardService/GetUserPrivacyMode"
            | "/agent.v1.AgentService/UpdateConversationMetadata"
            | "/aiserver.v1.AiService/GetServerConfig"
            | "/aiserver.v1.ServerConfigService/GetServerConfig"
            | "/aiserver.v1.AiService/AvailableModels"
            | "/agent.v1.AgentService/GetUsableModels"
            | "/aiserver.v1.AiService/GetUsableModels"
            | "/agent.v1.AgentService/GetDefaultModelForCli"
            | "/aiserver.v1.AiService/GetDefaultModelForCli"
            | "/aiserver.v1.AiService/GetDefaultModel"
            | "/aiserver.v1.AiService/GetDefaultModelNudgeData"
            | "/aiserver.v1.AuthService/GetEmail"
            | "/aiserver.v1.AuthService/GetUserMeta"
            | "/aiserver.v1.DashboardService/GetMe"
            | "/aiserver.v1.DashboardService/GetTeams"
            | "/aiserver.v1.DashboardService/GetUserProfile"
            | "/aiserver.v1.DashboardService/GetCurrentPeriodUsage"
            | "/aiserver.v1.DashboardService/GetUsageLimitStatusAndActiveGrants"
            | "/aiserver.v1.AiService/KnowledgeBaseAdd"
            | "/aiserver.v1.AiService/KnowledgeBaseList"
            | "/aiserver.v1.AiService/KnowledgeBaseUpdate"
            | "/aiserver.v1.AiService/KnowledgeBaseRemove"
            | "/aiserver.v1.AiService/WriteGitCommitMessage"
            | "/aiserver.v1.NetworkService/IsConnected"
            | "/aiserver.v1.AnalyticsService/BootstrapStatsig"
            | "/auth/full_stripe_profile"
            | "/auth/stripe_profile"
    )
}

/// Cursor's remote control stays here while BYOK is in front of Cursor: handed
/// back to Cursor's own service, it is told it is not registered and stays away.
fn should_route_locally(path: &str, tab_mode: TabMode) -> bool {
    is_local_path(path)
        || (is_tab_path(path) && tab_mode != TabMode::Direct)
        || path.starts_with(presence::PATH)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_remote_control_is_answered_here_whether_or_not_phones_are_let_in() {
        let path = "/aiserver.v1.RemoteAgentHostPresenceService/ConnectTarget";
        assert!(should_route_locally(path, TabMode::Direct));
    }

    /// Cursor answers over the body it opened the connection with, long after the response began.
    #[tokio::test]
    async fn a_remote_control_connection_carries_both_ways_at_once_over_http2() {
        use axum::{routing::post, Router};
        use bytes::Bytes;
        use futures_util::StreamExt;
        use hudsucker::{
            hyper::{client::conn::http2, Method, StatusCode},
            hyper_util::rt::{TokioExecutor, TokioIo},
            rustls::{pki_types::ServerName, ClientConfig, RootCertStore},
        };
        use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let backend = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = backend.local_addr().unwrap();
        let path = format!("{}ConnectTarget", presence::PATH);
        let echo = post(|request: axum::extract::Request| async {
            axum::body::Body::from_stream(request.into_body().into_data_stream())
        });
        let router = Router::new().route(&path, echo);
        tokio::spawn(async move { axum::serve(backend, router).await.unwrap() });

        let key = KeyPair::generate().unwrap();
        let mut params = CertificateParams::new(Vec::new()).unwrap();
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let certificate = params.self_signed(&key).unwrap();
        let mut roots = RootCertStore::empty();
        roots.add(certificate.der().clone()).unwrap();
        let ca = LoadedCa {
            issuer: Issuer::new(params, key),
        };
        let mut proxy = ProxyRuntime::default();
        let mode = Arc::new(RwLock::new(TabMode::Direct));
        let (_, port) = proxy.start(address, ca, 0, mode).await.unwrap();

        let mut tunnel = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        tunnel
            .write_all(b"CONNECT api2.cursor.sh:443 HTTP/1.1\r\nhost: api2.cursor.sh:443\r\n\r\n")
            .await
            .unwrap();
        let mut greeting = Vec::new();
        while !greeting.ends_with(b"\r\n\r\n") {
            greeting.push(tunnel.read_u8().await.unwrap());
        }
        assert!(greeting.starts_with(b"HTTP/1.1 200"));
        let provider = Arc::new(aws_lc_rs::default_provider());
        let mut tls = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
        tls.alpn_protocols = vec![b"h2".to_vec()];
        let tls = tokio_rustls::TlsConnector::from(Arc::new(tls))
            .connect(ServerName::try_from("api2.cursor.sh").unwrap(), tunnel)
            .await
            .unwrap();
        assert_eq!(tls.get_ref().1.alpn_protocol(), Some(b"h2".as_slice()));
        let (mut client, connection) = http2::handshake(TokioExecutor::new(), TokioIo::new(tls))
            .await
            .unwrap();
        tokio::spawn(connection);

        let (say, said) = tokio::sync::mpsc::unbounded_channel::<Result<Bytes>>();
        let said = tokio_stream::wrappers::UnboundedReceiverStream::new(said);
        let request = Request::builder()
            .method(Method::POST)
            .uri(format!("https://api2.cursor.sh{path}"))
            .body(axum::body::Body::from_stream(said))
            .unwrap();
        let wait = std::time::Duration::from_secs(5);
        let response = tokio::time::timeout(wait, client.send_request(request))
            .await
            .expect("the response waited for the whole request")
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let mut heard = axum::body::Body::new(response.into_body()).into_data_stream();
        for word in ["first", "second"] {
            say.send(Ok(word.into())).unwrap();
            let chunk = tokio::time::timeout(wait, heard.next()).await;
            assert_eq!(chunk.expect("held back").unwrap().unwrap(), word);
        }
        drop(say);
        let end = tokio::time::timeout(wait, heard.next()).await;
        assert!(end.unwrap().is_none_or(|rest| rest.unwrap().is_empty()));
        proxy.stop().await;
    }

    #[test]
    fn cursor_cli_transport_and_model_metadata_routes_stay_local() {
        for path in [
            "/aiserver.v1.AiService/GetServerConfig",
            "/aiserver.v1.ServerConfigService/GetServerConfig",
            "/agent.v1.AgentService/GetDefaultModelForCli",
            "/aiserver.v1.AiService/GetDefaultModelForCli",
            "/aiserver.v1.AiService/GetDefaultModel",
            "/aiserver.v1.AiService/GetDefaultModelNudgeData",
            "/aiserver.v1.AiService/AvailableDocs",
            "/aiserver.v1.DashboardService/GetEffectiveUserPlugins",
            "/aiserver.v1.DashboardService/GetUserPrivacyMode",
            "/aiserver.v1.AuthService/GetUserMeta",
            "/agent.v1.AgentService/UpdateConversationMetadata",
            "/auth/full_stripe_profile",
            "/auth/stripe_profile",
        ] {
            assert!(is_local_path(path), "{path} must not reach Cursor upstream");
        }
    }
}
