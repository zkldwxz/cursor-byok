//! Carries text messages between a computer and a phone, encrypted end to end.
mod ticket;

use std::{
    net::{Ipv4Addr, SocketAddr},
    time::Duration,
};

use iroh::{
    endpoint::{presets, Connection, Incoming, RecvStream, SendStream},
    Endpoint, EndpointAddr, RelayConfig, RelayMode, SecretKey, TransportAddr,
};
use tokio::{io::AsyncReadExt, sync::mpsc};

pub use ticket::{Relay, Ticket};

const ALPN: &[u8] = b"cursor-byok/remote/1";
const ONLINE_TIMEOUT: Duration = Duration::from_secs(10);
const DIAL_TIMEOUT: Duration = Duration::from_secs(20);
/// A conversation's history is the longest message.
const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;
/// Until it is known who speaks, and for everything a phone says.
const MAX_REQUEST_BYTES: usize = 256 * 1024;
const INBOX: usize = 64;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("pairing link is not valid: {0}")]
    Ticket(String),
    #[error("relay address is not valid: {0}")]
    Relay(String),
    #[error("remote connection: {0}")]
    Connection(String),
}

pub type Result<T> = std::result::Result<T, Error>;

fn lost(error: impl std::fmt::Display) -> Error {
    Error::Connection(error.to_string())
}

/// What makes a computer the one a phone paired with, whenever it comes back.
#[derive(Clone, Debug, PartialEq)]
pub struct Identity {
    pub key: [u8; 32],
    /// Where it was found before; zero for anywhere.
    pub port: u16,
}

impl Identity {
    pub fn generate() -> Self {
        Self {
            key: SecretKey::generate().to_bytes(),
            port: 0,
        }
    }
}

/// The computer's end; phones reach it by its key.
pub struct Host {
    endpoint: Endpoint,
    relay: Option<Relay>,
}

impl Host {
    pub async fn bind(relay: Option<Relay>, identity: &Identity) -> Result<Self> {
        let key = SecretKey::from_bytes(&identity.key);
        let endpoint = match endpoint(relay.as_ref(), key.clone(), identity.port).await {
            Err(error) if identity.port != 0 => {
                // Taken by something else: phones that use the relay still find it.
                tracing::warn!(%error, port = identity.port, "remote port is not free");
                endpoint(relay.as_ref(), key, 0).await?
            }
            bound => bound?,
        };
        if relay.is_some()
            && tokio::time::timeout(ONLINE_TIMEOUT, endpoint.online())
                .await
                .is_err()
        {
            // The same network may still do.
            tracing::warn!("remote relay was not reached");
        }
        Ok(Self { endpoint, relay })
    }

    /// The port phones on the same network find it at.
    pub fn port(&self) -> Option<u16> {
        let bound = self.endpoint.bound_sockets();
        bound
            .iter()
            .find(|socket| socket.is_ipv4())
            .map(|it| it.port())
    }

    /// Where this computer is now, and the code that pairs a phone with it.
    pub fn ticket(&self, code: String) -> Ticket {
        Ticket {
            key: self.endpoint.id(),
            code,
            addresses: self.endpoint.addr().ip_addrs().copied().collect(),
            relay: self.relay.clone(),
        }
    }

    /// The next phone at the door; `None` once closed.
    pub async fn accept(&self) -> Option<Knock> {
        self.endpoint.accept().await.map(Knock)
    }

    pub async fn close(&self) {
        self.endpoint.close().await;
    }
}

/// A phone at the door. Answered apart from accepting, so a slow one keeps no other waiting.
pub struct Knock(Incoming);

impl Knock {
    /// Whoever it is has proved nothing yet.
    pub async fn answer(self) -> Result<Link> {
        let link = async {
            let connection = self.0.await.map_err(lost)?;
            let (send, receive) = connection.accept_bi().await.map_err(lost)?;
            Link::start(connection, send, receive, MAX_REQUEST_BYTES).await
        };
        tokio::time::timeout(DIAL_TIMEOUT, link)
            .await
            .map_err(|_| lost("the phone took too long to connect"))?
    }
}

/// The phone's end.
pub struct Phone {
    endpoint: Endpoint,
}

impl Phone {
    pub async fn bind(relay: Option<&Relay>) -> Result<Self> {
        Ok(Self {
            endpoint: endpoint(relay, SecretKey::generate(), 0).await?,
        })
    }

    pub async fn close(&self) {
        self.endpoint.close().await;
    }

    pub async fn dial(&self, ticket: &Ticket) -> Result<Link> {
        let mut addresses: Vec<_> = ticket
            .addresses
            .iter()
            .copied()
            .map(TransportAddr::Ip)
            .collect();
        if let Some(relay) = &ticket.relay {
            addresses.push(TransportAddr::Relay(relay.url()?));
        }
        let address = EndpointAddr::from_parts(ticket.key, addresses);
        let dial = async {
            let connection = self.endpoint.connect(address, ALPN).await.map_err(lost)?;
            let (send, receive) = connection.open_bi().await.map_err(lost)?;
            Link::start(connection, send, receive, MAX_MESSAGE_BYTES).await
        };
        tokio::time::timeout(DIAL_TIMEOUT, dial)
            .await
            .map_err(|_| lost("the computer did not answer"))?
    }
}

async fn endpoint(relay: Option<&Relay>, key: SecretKey, port: u16) -> Result<Endpoint> {
    let mode = match relay {
        Some(relay) => {
            let mut config = RelayConfig::new(relay.url()?, None);
            if let Some(token) = &relay.token {
                config = config.with_auth_token(token.clone());
            }
            RelayMode::Custom([config].into_iter().collect())
        }
        None => RelayMode::Disabled,
    };
    Endpoint::builder(presets::Minimal)
        .secret_key(key)
        .alpns(vec![ALPN.to_vec()])
        .relay_mode(mode)
        .bind_addr(SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)))
        .map_err(lost)?
        .bind()
        .await
        .map_err(lost)
}

/// One conversation between a phone and the computer.
pub struct Link {
    connection: Connection,
    send: SendStream,
    inbox: mpsc::Receiver<String>,
}

impl Link {
    async fn start(
        connection: Connection,
        mut send: SendStream,
        mut receive: RecvStream,
        limit: usize,
    ) -> Result<Self> {
        // A stream exists for the other side only once something was written to it.
        send.write_all(&[0; 4]).await.map_err(lost)?;
        let (heard, inbox) = mpsc::channel(INBOX);
        // Reading here lets `receive` be given up at any point without losing part of a message.
        tokio::spawn(async move {
            while let Ok(message) = read(&mut receive, limit).await {
                if !message.is_empty() && heard.send(message).await.is_err() {
                    return;
                }
            }
        });
        Ok(Self {
            connection,
            send,
            inbox,
        })
    }

    /// `None` once the other side is gone. Safe to cancel.
    pub async fn receive(&mut self) -> Option<String> {
        self.inbox.recv().await
    }

    pub async fn send(&mut self, message: &str) -> Result<()> {
        let length = u32::try_from(message.len()).map_err(lost)?;
        self.send
            .write_all(&length.to_be_bytes())
            .await
            .map_err(lost)?;
        self.send.write_all(message.as_bytes()).await.map_err(lost)
    }

    pub fn route(&self) -> Route {
        Route(self.connection.clone())
    }
}

/// The way a link's messages go, which may change while it lasts.
#[derive(Clone)]
pub struct Route(Connection);

impl Route {
    /// Whether straight to the other side, rather than through the relay.
    pub fn direct(&self) -> bool {
        self.0
            .paths()
            .into_iter()
            .any(|path| path.is_selected() && path.is_ip())
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        // What was sent last must still arrive: closing the connection would discard it.
        let _ = self.send.finish();
        let connection = self.connection.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                tokio::time::sleep(Duration::from_secs(2)).await;
                connection.close(0u8.into(), b"done");
            });
        }
    }
}

async fn read(receive: &mut RecvStream, limit: usize) -> Result<String> {
    let length = receive.read_u32().await.map_err(lost)? as usize;
    if length > limit {
        return Err(lost(format!("a message of {length} bytes is too long")));
    }
    let mut message = vec![0; length];
    receive.read_exact(&mut message).await.map_err(lost)?;
    String::from_utf8(message).map_err(lost)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn pair() -> (Host, Phone, Link, Link) {
        let host = Host::bind(None, &Identity::generate()).await.unwrap();
        let phone = Phone::bind(None).await.unwrap();
        let ticket = host.ticket("code".into());
        let answer = async { host.accept().await.unwrap().answer().await };
        let (theirs, ours) = tokio::join!(answer, phone.dial(&ticket));
        let (theirs, ours) = (theirs.unwrap(), ours.unwrap());
        (host, phone, theirs, ours)
    }

    #[tokio::test]
    async fn messages_go_both_ways_whole_and_in_order() {
        let (_host, _phone, mut computer, mut phone) = pair().await;
        let long = "长".repeat(400_000);
        phone.send("one").await.unwrap();
        phone.send("two").await.unwrap();
        computer.send(&long).await.unwrap();
        assert_eq!(computer.receive().await.unwrap(), "one");
        assert_eq!(computer.receive().await.unwrap(), "two");
        assert_eq!(phone.receive().await.unwrap(), long);
        assert!(phone.route().direct());
    }

    #[tokio::test]
    async fn the_last_message_arrives_before_the_end() {
        let (_host, _phone, mut computer, mut phone) = pair().await;
        computer.send("unpaired").await.unwrap();
        drop(computer);
        assert_eq!(phone.receive().await.unwrap(), "unpaired");
        assert!(phone.receive().await.is_none());
    }

    #[tokio::test]
    async fn a_phone_that_says_too_much_is_cut_off() {
        let (_host, _phone, mut computer, mut phone) = pair().await;
        let _ = phone.send(&"x".repeat(MAX_REQUEST_BYTES + 1)).await;
        assert!(computer.receive().await.is_none());
    }

    #[tokio::test]
    async fn a_phone_reaches_only_the_computer_whose_key_it_was_given() {
        let host = Host::bind(None, &Identity::generate()).await.unwrap();
        let other = Host::bind(None, &Identity::generate()).await.unwrap();
        let phone = Phone::bind(None).await.unwrap();
        let mut ticket = host.ticket("code".into());
        ticket.addresses = other.ticket(String::new()).addresses;
        let dialled = tokio::time::timeout(Duration::from_secs(4), phone.dial(&ticket)).await;
        assert!(!matches!(dialled, Ok(Ok(_))));
    }

    #[tokio::test]
    async fn a_phone_that_connects_and_says_nothing_keeps_no_other_waiting() {
        let host = Host::bind(None, &Identity::generate()).await.unwrap();
        let ticket = host.ticket("code".into());
        let silent = Phone::bind(None).await.unwrap();
        let address = EndpointAddr::from_parts(
            ticket.key,
            ticket.addresses.iter().copied().map(TransportAddr::Ip),
        );
        let connecting = tokio::spawn({
            let endpoint = silent.endpoint.clone();
            async move { endpoint.connect(address, ALPN).await }
        });
        tokio::spawn(host.accept().await.unwrap().answer());
        let _connected = connecting.await.unwrap().unwrap();
        let phone = Phone::bind(None).await.unwrap();
        let answer = async { host.accept().await.unwrap().answer().await };
        let (theirs, ours) = tokio::join!(answer, phone.dial(&ticket));
        assert!(theirs.is_ok() && ours.is_ok());
    }

    #[tokio::test]
    async fn a_computer_that_comes_back_is_found_with_the_same_ticket() {
        let mut identity = Identity::generate();
        let host = Host::bind(None, &identity).await.unwrap();
        identity.port = host.port().unwrap();
        let ticket = host.ticket(String::new());
        host.close().await;
        let host = Host::bind(None, &identity).await.unwrap();
        let phone = Phone::bind(None).await.unwrap();
        let answer = async { host.accept().await.unwrap().answer().await };
        let (theirs, ours) = tokio::join!(answer, phone.dial(&ticket));
        assert!(theirs.is_ok() && ours.is_ok());
    }

    #[tokio::test]
    async fn a_computer_whose_port_was_taken_still_starts() {
        let mut identity = Identity::generate();
        let first = Host::bind(None, &identity).await.unwrap();
        identity.port = first.port().unwrap();
        let second = Host::bind(None, &identity).await.unwrap();
        assert_ne!(second.port(), first.port());
    }

    #[tokio::test]
    async fn closing_the_computer_ends_accepting() {
        let host = Host::bind(None, &Identity::generate()).await.unwrap();
        host.close().await;
        assert!(host.accept().await.is_none());
    }
}
