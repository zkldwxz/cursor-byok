//! Owns reusable outbound HTTP clients configured from persisted proxy settings.

use std::{
    hash::{Hash, Hasher},
    sync::Arc,
    time::{Duration, Instant},
};

use tokio::sync::RwLock;

use crate::{
    store::{ProxySettingsSecret, Store},
    Error, Result,
};

const LOCAL_NO_PROXY: &str = "localhost,127.0.0.0/8,::1";

/// How often cached clients re-check the effective proxy configuration.
///
/// reqwest resolves the system proxy once when a `Client` is built and freezes
/// it into the connector (`hyper-util` `Matcher::from_system`), so a running
/// process never notices when the user toggles a system-wide proxy (Clash
/// on/off, a port change): every request keeps hitting the stale proxy and
/// fails within milliseconds until the server is restarted. Comparing a cheap
/// fingerprint on this interval lets stale clients be rebuilt within seconds.
const FINGERPRINT_CHECK_INTERVAL: Duration = Duration::from_secs(2);

/// How long a TCP/TLS handshake may take before the request fails. Provider
/// calls keep their own much longer total timeout; this only bounds a dead
/// host so the caller sees an error instead of a hung UI.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub struct NetworkClients {
    store: Store,
    cache: Arc<RwLock<ClientCache>>,
    /// Reads the effective system proxy values, `None` when they cannot be
    /// read. Injectable so tests do not have to mutate process-wide state.
    system_proxy_source: Arc<dyn Fn() -> Option<SystemProxyConfig> + Send + Sync>,
}

#[derive(Default)]
struct ClientCache {
    default: Option<reqwest::Client>,
    cursor: Option<reqwest::Client>,
    provider: Option<(Duration, reqwest::Client)>,
    magpie: Option<(Duration, reqwest::Client)>,
    /// Fingerprint of the proxy configuration the cached clients were built
    /// with, and when the fingerprint was last recomputed.
    fingerprint: Option<(u64, Instant)>,
}

impl NetworkClients {
    pub fn new(store: Store) -> Self {
        Self {
            store,
            cache: Arc::new(RwLock::new(ClientCache::default())),
            system_proxy_source: Arc::new(system_proxy_config),
        }
    }

    pub async fn default_client(&self) -> Result<reqwest::Client> {
        self.invalidate_if_proxy_changed().await;
        if let Some(client) = self.cache.read().await.default.clone() {
            return Ok(client);
        }
        let mut cache = self.cache.write().await;
        if let Some(client) = cache.default.clone() {
            return Ok(client);
        }
        let client = client_builder(&self.store).await?.build()?;
        cache.default = Some(client.clone());
        Ok(client)
    }

    pub async fn cursor_client(&self) -> Result<reqwest::Client> {
        self.invalidate_if_proxy_changed().await;
        if let Some(client) = self.cache.read().await.cursor.clone() {
            return Ok(client);
        }
        let mut cache = self.cache.write().await;
        if let Some(client) = cache.cursor.clone() {
            return Ok(client);
        }
        let client = client_builder(&self.store)
            .await?
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        cache.cursor = Some(client.clone());
        Ok(client)
    }

    pub async fn provider_client(&self, timeout: Duration) -> Result<reqwest::Client> {
        self.invalidate_if_proxy_changed().await;
        if let Some((_, client)) = self
            .cache
            .read()
            .await
            .provider
            .as_ref()
            .filter(|(cached_timeout, _)| *cached_timeout == timeout)
        {
            return Ok(client.clone());
        }
        let mut cache = self.cache.write().await;
        if let Some((_, client)) = cache
            .provider
            .as_ref()
            .filter(|(cached_timeout, _)| *cached_timeout == timeout)
        {
            return Ok(client.clone());
        }
        let client = client_builder(&self.store)
            .await?
            .timeout(timeout)
            .build()?;
        cache.provider = Some((timeout, client.clone()));
        Ok(client)
    }

    pub(crate) async fn model_client(
        &self,
        model: &crate::model::ModelConfig,
        timeout: Duration,
    ) -> Result<reqwest::Client> {
        if model.gateway_capabilities.is_none() {
            return self.provider_client(timeout).await;
        }
        let client = {
            let mut cache = self.cache.write().await;
            if cache.magpie.as_ref().is_none_or(|(t, _)| *t != timeout) {
                cache.magpie = Some((timeout, crate::provider::magpie::local_client(timeout)?));
            }
            cache.magpie.as_ref().unwrap().1.clone()
        };
        // A stopped gateway's port can be reused by an unrelated local service.
        crate::provider::magpie::verify_gateway(&client, &model.base_url).await?;
        Ok(client)
    }

    pub async fn invalidate(&self) {
        *self.cache.write().await = ClientCache::default();
    }

    /// Drops the cached clients when the effective proxy configuration changed
    /// since they were built. Rate-limited to one fingerprint computation per
    /// `FINGERPRINT_CHECK_INTERVAL`; a fingerprint that cannot be read keeps
    /// the current cache rather than flushing it spuriously.
    async fn invalidate_if_proxy_changed(&self) {
        {
            let cache = self.cache.read().await;
            if let Some((_, checked_at)) = cache.fingerprint {
                if checked_at.elapsed() < FINGERPRINT_CHECK_INTERVAL {
                    return;
                }
            }
        }
        let Some(fingerprint) = self.proxy_fingerprint().await else {
            return;
        };
        let mut cache = self.cache.write().await;
        match cache.fingerprint {
            Some((cached, _)) if cached == fingerprint => {
                cache.fingerprint = Some((fingerprint, Instant::now()));
            }
            previous => {
                if previous.is_some() {
                    tracing::info!("proxy configuration changed; rebuilding cached HTTP clients");
                }
                *cache = ClientCache {
                    fingerprint: Some((fingerprint, Instant::now())),
                    ..ClientCache::default()
                };
            }
        }
    }

    /// Hashes everything that decides how outbound clients route requests:
    /// the persisted custom proxy settings and, while no custom proxy is
    /// configured, the system proxy values `hyper-util`'s
    /// `Matcher::from_system` freezes into each reqwest client at build time
    /// (a custom `reqwest::Proxy` disables that system resolution, so it must
    /// not trigger rebuilds then). Returns `None` when the fingerprint cannot
    /// be read, in which case the caller keeps the current cache.
    async fn proxy_fingerprint(&self) -> Option<u64> {
        let settings = self.store.proxy_settings_secret().await.ok()?;
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        settings.mode.is_custom().hash(&mut hasher);
        if settings.mode.is_custom() {
            settings.address.hash(&mut hasher);
            settings.auth_enabled.hash(&mut hasher);
            settings.username.hash(&mut hasher);
            settings.password.hash(&mut hasher);
        } else {
            (self.system_proxy_source)()?.hash(&mut hasher);
        }
        Some(hasher.finish())
    }
}

/// The system proxy values frozen into a client by `hyper-util`'s
/// `Matcher::from_system`, after the OS configuration has been merged into the
/// environment variables exactly as its builder does: OS entries fill only
/// the slots the environment left empty.
#[derive(Clone, Debug, Default, Hash, PartialEq, Eq)]
struct SystemProxyConfig {
    all: String,
    http: String,
    https: String,
    no: String,
}

/// The first non-empty lookup of each variable pair, mirroring `hyper-util`'s
/// `get_first_env` (an unset variable and an empty one both yield `""`).
fn first_env(names: &[&str]) -> String {
    names
        .iter()
        .find_map(|name| std::env::var(name).ok())
        .unwrap_or_default()
}

#[cfg_attr(not(any(target_os = "macos", windows)), allow(unused_mut))]
fn system_proxy_config() -> Option<SystemProxyConfig> {
    let mut config = SystemProxyConfig {
        all: first_env(&["ALL_PROXY", "all_proxy"]),
        http: first_env(&["HTTP_PROXY", "http_proxy"]),
        https: first_env(&["HTTPS_PROXY", "https_proxy"]),
        no: first_env(&["NO_PROXY", "no_proxy"]),
    };
    #[cfg(target_os = "macos")]
    macos::merge_system_proxies(&mut config);
    #[cfg(windows)]
    windows::merge_system_proxies(&mut config);
    Some(config)
}

/// macOS system proxy entries, merged exactly as `hyper-util`'s `mac` module
/// does: HTTP and HTTPS slots only, only where the environment is empty, and
/// a store that cannot be read contributes nothing. Enabled entries parse to
/// `host:port` the same way before hashing.
#[cfg(target_os = "macos")]
mod macos {
    use system_configuration::core_foundation::base::CFType;
    use system_configuration::core_foundation::dictionary::CFDictionary;
    use system_configuration::core_foundation::number::CFNumber;
    use system_configuration::core_foundation::string::{CFString, CFStringRef};
    use system_configuration::dynamic_store::SCDynamicStoreBuilder;
    use system_configuration::sys::schema_definitions::{
        kSCPropNetProxiesHTTPEnable, kSCPropNetProxiesHTTPPort, kSCPropNetProxiesHTTPProxy,
        kSCPropNetProxiesHTTPSEnable, kSCPropNetProxiesHTTPSPort, kSCPropNetProxiesHTTPSProxy,
    };

    use super::SystemProxyConfig;

    pub fn merge_system_proxies(config: &mut SystemProxyConfig) {
        let Some(store) = SCDynamicStoreBuilder::new("cursor-byok").build() else {
            return;
        };
        let Some(proxies) = store.get_proxies() else {
            return;
        };
        if config.http.is_empty() {
            if let Some(http) = parse_setting(
                &proxies,
                unsafe { kSCPropNetProxiesHTTPEnable },
                unsafe { kSCPropNetProxiesHTTPProxy },
                unsafe { kSCPropNetProxiesHTTPPort },
            ) {
                config.http = http;
            }
        }
        if config.https.is_empty() {
            if let Some(https) = parse_setting(
                &proxies,
                unsafe { kSCPropNetProxiesHTTPSEnable },
                unsafe { kSCPropNetProxiesHTTPSProxy },
                unsafe { kSCPropNetProxiesHTTPSPort },
            ) {
                config.https = https;
            }
        }
    }

    fn parse_setting(
        proxies: &CFDictionary<CFString, CFType>,
        enabled_key: CFStringRef,
        host_key: CFStringRef,
        port_key: CFStringRef,
    ) -> Option<String> {
        let enabled = proxies
            .find(enabled_key)
            .and_then(|flag| flag.downcast::<CFNumber>())
            .and_then(|flag| flag.to_i32())
            .unwrap_or(0)
            == 1;
        if !enabled {
            return None;
        }
        let host = proxies
            .find(host_key)
            .and_then(|host| host.downcast::<CFString>())
            .map(|host| host.to_string());
        let port = proxies
            .find(port_key)
            .and_then(|port| port.downcast::<CFNumber>())
            .and_then(|port| port.to_i32());
        match (host, port) {
            (Some(host), Some(port)) => Some(format!("{host}:{port}")),
            (Some(host), None) => Some(host),
            (None, _) => None,
        }
    }
}

/// Windows system proxy entries, merged exactly as `hyper-util`'s `win`
/// module does: `ProxyEnable` off or a key that cannot be opened contributes
/// nothing, `ProxyServer` fills both the HTTP and HTTPS slots the environment
/// left empty, and `ProxyOverride` is normalized (`;` to `,`, `*.` prefixes
/// stripped) before filling an empty `no` slot.
#[cfg(windows)]
mod windows {
    use super::SystemProxyConfig;

    pub fn merge_system_proxies(config: &mut SystemProxyConfig) {
        let settings = if let Ok(settings) = windows_registry::CURRENT_USER
            .open("Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings")
        {
            settings
        } else {
            return;
        };
        if settings.get_u32("ProxyEnable").unwrap_or(0) == 0 {
            return;
        }
        if let Ok(server) = settings.get_string("ProxyServer") {
            if config.http.is_empty() {
                config.http = server.clone();
            }
            if config.https.is_empty() {
                config.https = server;
            }
        }
        if config.no.is_empty() {
            if let Ok(bypass) = settings.get_string("ProxyOverride") {
                config.no = bypass
                    .split(';')
                    .map(str::trim)
                    .collect::<Vec<&str>>()
                    .join(",")
                    .replace("*.", "");
            }
        }
    }
}

pub async fn client_builder(store: &Store) -> Result<reqwest::ClientBuilder> {
    let settings = store.proxy_settings_secret().await?;
    // Use the platform TLS stack for compatibility with provider gateways that
    // only offer legacy TLS 1.2 cipher suites unsupported by rustls.
    //
    // A dead upstream must fail fast: without a connect timeout the Cursor
    // backend forwards hang on the OS SYN timeout (logs show a status request
    // taking 117 seconds), and the Cursor UI waits with them.
    let mut builder = reqwest::Client::builder()
        .use_native_tls()
        .connect_timeout(CONNECT_TIMEOUT);
    if settings.mode.is_custom() {
        builder = builder.proxy(custom_proxy(&settings)?);
    }
    Ok(builder)
}

pub async fn client(store: &Store) -> Result<reqwest::Client> {
    Ok(client_builder(store).await?.build()?)
}

fn custom_proxy(settings: &ProxySettingsSecret) -> Result<reqwest::Proxy> {
    let mut proxy = reqwest::Proxy::all(&settings.address)?
        .no_proxy(reqwest::NoProxy::from_string(LOCAL_NO_PROXY));
    if settings.auth_enabled {
        proxy = proxy.basic_auth(&settings.username, &settings.password);
    }
    Ok(proxy)
}

pub fn reject_self_proxy(address: &str, local_proxy_port: u16) -> Result<()> {
    if local_proxy_port == 0 {
        return Ok(());
    }
    let url = url::Url::parse(address)
        .map_err(|error| Error::Config(format!("invalid proxy address: {error}")))?;
    if url.port_or_known_default() == Some(local_proxy_port) && url_host_is_loopback(&url) {
        return Err(Error::Config(
            "proxy address cannot point to the Cursor BYOK local proxy".into(),
        ));
    }
    Ok(())
}

fn url_host_is_loopback(url: &url::Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(host)) => {
            host.trim_end_matches('.').eq_ignore_ascii_case("localhost")
        }
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;
    use crate::store::ProxyMode;

    async fn test_store() -> (tempfile::TempDir, Store) {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::connect(&format!(
            "sqlite://{}",
            directory.path().join("test.db").display()
        ))
        .await
        .unwrap();
        (directory, store)
    }

    /// A `NetworkClients` whose system proxy values come from `config` instead
    /// of the real environment and OS.
    fn clients_with_system_proxy(
        store: Store,
        config: Arc<Mutex<Option<SystemProxyConfig>>>,
    ) -> NetworkClients {
        NetworkClients {
            store,
            cache: Arc::new(RwLock::new(ClientCache::default())),
            system_proxy_source: Arc::new(move || config.lock().unwrap().clone()),
        }
    }

    fn stable_source() -> (Arc<Mutex<Option<SystemProxyConfig>>>, SystemProxyConfig) {
        let config = SystemProxyConfig {
            all: String::new(),
            http: "127.0.0.1:7890".into(),
            https: "127.0.0.1:7890".into(),
            no: String::new(),
        };
        (Arc::new(Mutex::new(Some(config.clone()))), config)
    }

    /// Lets the next fingerprint check run immediately instead of waiting out
    /// `FINGERPRINT_CHECK_INTERVAL`.
    async fn expire_fingerprint_check(clients: &NetworkClients) {
        let mut cache = clients.cache.write().await;
        if let Some((fingerprint, checked_at)) = cache.fingerprint {
            cache.fingerprint = Some((
                fingerprint,
                checked_at
                    .checked_sub(FINGERPRINT_CHECK_INTERVAL * 2)
                    .unwrap_or(checked_at),
            ));
        }
    }

    #[tokio::test]
    async fn keeps_the_cached_clients_while_the_proxy_configuration_is_unchanged() {
        let (_directory, store) = test_store().await;
        let (source, _config) = stable_source();
        let clients = clients_with_system_proxy(store, source);
        clients.default_client().await.unwrap();
        clients.cursor_client().await.unwrap();
        let primed_fingerprint = clients.cache.read().await.fingerprint.unwrap().0;

        expire_fingerprint_check(&clients).await;
        clients.default_client().await.unwrap();

        let cache = clients.cache.read().await;
        assert!(cache.cursor.is_some(), "unchanged config dropped the cache");
        assert_eq!(cache.fingerprint.unwrap().0, primed_fingerprint);
    }

    #[tokio::test]
    async fn drops_the_cached_clients_when_the_proxy_configuration_changes() {
        let (_directory, store) = test_store().await;
        let (source, mut config) = stable_source();
        let clients = clients_with_system_proxy(store, source.clone());
        clients.default_client().await.unwrap();
        clients.cursor_client().await.unwrap();
        let primed_fingerprint = clients.cache.read().await.fingerprint.unwrap().0;

        config.http = "127.0.0.1:7897".into();
        *source.lock().unwrap() = Some(config);
        expire_fingerprint_check(&clients).await;
        clients.default_client().await.unwrap();

        let cache = clients.cache.read().await;
        assert!(
            cache.cursor.is_none(),
            "changed config kept the stale cached clients"
        );
        assert_ne!(cache.fingerprint.unwrap().0, primed_fingerprint);
    }

    #[tokio::test]
    async fn keeps_the_cache_when_the_fingerprint_cannot_be_read() {
        let (_directory, store) = test_store().await;
        let (source, _config) = stable_source();
        let clients = clients_with_system_proxy(store, source.clone());
        clients.default_client().await.unwrap();
        clients.cursor_client().await.unwrap();
        let primed_fingerprint = clients.cache.read().await.fingerprint.unwrap().0;

        *source.lock().unwrap() = None;
        expire_fingerprint_check(&clients).await;
        clients.default_client().await.unwrap();

        let cache = clients.cache.read().await;
        assert!(
            cache.cursor.is_some(),
            "an unreadable fingerprint flushed the cache"
        );
        assert_eq!(cache.fingerprint.unwrap().0, primed_fingerprint);
    }

    #[tokio::test]
    async fn skips_the_fingerprint_check_within_the_check_interval() {
        let (_directory, store) = test_store().await;
        let (source, mut config) = stable_source();
        let clients = clients_with_system_proxy(store, source.clone());
        clients.default_client().await.unwrap();
        clients.cursor_client().await.unwrap();

        // The check ran when the cache was primed, well within the interval,
        // so this change must not be picked up yet.
        config.http = "127.0.0.1:7897".into();
        *source.lock().unwrap() = Some(config);
        clients.default_client().await.unwrap();

        assert!(
            clients.cache.read().await.cursor.is_some(),
            "a check inside the interval dropped the cache"
        );
    }

    #[test]
    fn system_proxy_config_tracks_the_proxy_environment_variables() {
        static ENV_LOCK: Mutex<()> = Mutex::new(());
        let _guard = ENV_LOCK.lock().unwrap();
        let previous = std::env::var("NO_PROXY").ok();
        std::env::set_var("NO_PROXY", "cursor-byok-fingerprint-test.invalid");
        let changed = system_proxy_config();
        match previous {
            Some(value) => std::env::set_var("NO_PROXY", value),
            None => std::env::remove_var("NO_PROXY"),
        }
        let restored = system_proxy_config();

        let changed = changed.unwrap();
        assert_eq!(changed.no, "cursor-byok-fingerprint-test.invalid");
        assert_ne!(changed, restored.unwrap());
    }

    fn custom_settings(address: String) -> ProxySettingsSecret {
        ProxySettingsSecret {
            mode: ProxyMode::Custom,
            address,
            auth_enabled: false,
            username: String::new(),
            password: String::new(),
        }
    }

    #[test]
    fn rejects_only_own_loopback_proxy_port() {
        for address in [
            "http://localhost:15721",
            "http://localhost.:15721",
            "http://127.0.0.2:15721",
            "http://[::1]:15721",
        ] {
            assert!(reject_self_proxy(address, 15721).is_err(), "{address}");
        }
        assert!(reject_self_proxy("http://127.0.0.1:7890", 15721).is_ok());
        assert!(reject_self_proxy("http://192.168.1.2:15721", 15721).is_ok());
        assert!(reject_self_proxy("http://127.0.0.1:15721", 0).is_ok());
    }

    #[tokio::test]
    async fn custom_proxy_bypasses_loopback_destinations() {
        let proxy_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_address = proxy_listener.local_addr().unwrap();
        let target_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target_address = target_listener.local_addr().unwrap();
        let target = tokio::spawn(async move {
            let (mut socket, _) = target_listener.accept().await.unwrap();
            let mut request = [0_u8; 1024];
            let _ = socket.read(&mut request).await.unwrap();
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .await
                .unwrap();
        });
        let settings = custom_settings(format!("http://{proxy_address}"));
        let client = reqwest::Client::builder()
            .proxy(custom_proxy(&settings).unwrap())
            .build()
            .unwrap();

        let body = client
            .get(format!("http://{target_address}"))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();

        assert_eq!(body, "ok");
        target.await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(50), proxy_listener.accept())
                .await
                .is_err(),
            "loopback destination unexpectedly reached the configured proxy"
        );
    }
}
