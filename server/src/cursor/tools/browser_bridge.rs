//! Relays MCP calls to the browser extension on this desktop.

use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot, Mutex};
use uuid::Uuid;

/// Longer than one poll, and than one call, during which the extension does not poll.
const AWAY: Duration = Duration::from_secs(60);

pub(crate) static BRIDGE: Bridge = Bridge::new(AWAY);

pub(crate) struct Bridge {
    token: OnceLock<String>,
    browser: Mutex<Option<Arc<Browser>>>,
    away: Duration,
}

/// One browser profile; only one at a time, or a call would land in whichever asked first.
struct Browser {
    id: String,
    seen: parking_lot::Mutex<Instant>,
    send: mpsc::Sender<BrowserRequest>,
    queue: Mutex<mpsc::Receiver<BrowserRequest>>,
    pending: Mutex<HashMap<String, oneshot::Sender<BrowserReply>>>,
}

impl Browser {
    fn new(id: &str) -> Arc<Self> {
        let (send, queue) = mpsc::channel(16);
        Arc::new(Self {
            id: id.into(),
            seen: parking_lot::Mutex::new(Instant::now()),
            send,
            queue: Mutex::new(queue),
            pending: Mutex::new(HashMap::new()),
        })
    }

    fn heard(&self) {
        *self.seen.lock() = Instant::now();
    }

    async fn next(&self) -> Option<BrowserRequest> {
        let mut queue = self.queue.lock().await;
        while let Some(request) = queue.recv().await {
            if self.pending.lock().await.contains_key(&request.id) {
                return Some(request);
            }
        }
        None
    }
}

#[derive(Serialize)]
pub(crate) struct BrowserRequest {
    id: String,
    method: String,
    arguments: Value,
    expires_ms: u64,
}

#[derive(Deserialize)]
pub(crate) struct BrowserReply {
    pub(crate) id: String,
    pub(crate) result: Option<Value>,
    pub(crate) error: Option<String>,
}

pub(crate) enum Next {
    /// The token is not this run's.
    Refused,
    /// Another browser is in use.
    Busy,
    Nothing,
    Request(BrowserRequest),
}

impl Bridge {
    const fn new(away: Duration) -> Self {
        Self {
            token: OnceLock::new(),
            browser: Mutex::const_new(None),
            away,
        }
    }

    /// What the extension shows to be let in; another one each time BYOK starts.
    pub(crate) fn token(&self) -> &str {
        self.token
            .get_or_init(|| Uuid::new_v4().simple().to_string())
    }

    pub(crate) async fn connected(&self) -> bool {
        self.current().await.is_some()
    }

    async fn current(&self) -> Option<Arc<Browser>> {
        let browser = self.browser.lock().await;
        browser.as_ref().filter(|it| self.is_here(it)).cloned()
    }

    fn is_here(&self, browser: &Browser) -> bool {
        browser.seen.lock().elapsed() < self.away
    }

    /// The browser of this id, once it is the one in use.
    async fn browser(&self, id: &str, take: bool) -> Option<Arc<Browser>> {
        let mut current = self.browser.lock().await;
        if let Some(browser) = current.as_ref() {
            if browser.id == id {
                return Some(browser.clone());
            }
            if !take && self.is_here(browser) {
                return None;
            }
            browser.pending.lock().await.clear();
        }
        let browser = Browser::new(id);
        *current = Some(browser.clone());
        Some(browser)
    }

    /// Connects the browser of this id; `take` turns to it from another.
    pub(crate) async fn enter(&self, token: &str, id: &str, take: bool) -> Next {
        if token != self.token() {
            return Next::Refused;
        }
        match self.browser(id, take).await {
            Some(browser) => {
                browser.heard();
                Next::Nothing
            }
            None => Next::Busy,
        }
    }

    /// Waits for a request to the browser of this id.
    pub(crate) async fn next(&self, token: &str, id: &str) -> Next {
        if token != self.token() {
            return Next::Refused;
        }
        let Some(browser) = self.browser(id, false).await else {
            return Next::Busy;
        };
        browser.heard();
        let request = tokio::time::timeout(Duration::from_secs(20), browser.next()).await;
        browser.heard();
        match request {
            Ok(Some(request)) => Next::Request(request),
            _ => Next::Nothing,
        }
    }

    pub(crate) async fn reply(&self, token: &str, reply: BrowserReply) -> bool {
        if token != self.token() {
            return false;
        }
        let Some(browser) = self.browser.lock().await.clone() else {
            return false;
        };
        let pending = browser.pending.lock().await.remove(&reply.id);
        pending.is_some_and(|sender| {
            browser.heard();
            sender.send(reply).is_ok()
        })
    }

    pub(crate) async fn call(
        &self,
        method: String,
        arguments: Value,
    ) -> std::result::Result<Value, String> {
        let browser = self.current().await.ok_or(
            "Chrome is not connected; it must be running with the Cursor DevTools Bridge extension.",
        )?;
        let id = Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        let ttl = Duration::from_secs(45);
        browser.pending.lock().await.insert(id.clone(), tx);
        let request = BrowserRequest {
            id: id.clone(),
            method,
            arguments,
            expires_ms: super::runtime::now_ms() + ttl.as_millis() as u64,
        };
        if let Err(error) = browser.send.try_send(request) {
            browser.pending.lock().await.remove(&id);
            return Err(format!("Chrome bridge queue: {error}"));
        }
        let result = tokio::time::timeout(ttl, rx).await;
        browser.pending.lock().await.remove(&id);
        let reply = result
            .map_err(|_| "Chrome did not answer; check that it is still running.")?
            .map_err(|_| "Another browser was turned to before the tool completed.")?;
        match (reply.result, reply.error) {
            (Some(result), None) => Ok(result),
            (None, Some(error)) => Err(error),
            _ => Err("Invalid Chrome bridge reply".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn answer(id: &str, result: Value) -> BrowserReply {
        BrowserReply {
            id: id.into(),
            result: Some(result),
            error: None,
        }
    }

    /// Polls until the bridge takes the browser for connected.
    async fn connect(bridge: &Arc<Bridge>, id: &'static str) -> tokio::task::JoinHandle<Next> {
        let poll = tokio::spawn({
            let bridge = bridge.clone();
            async move { bridge.next(bridge.token(), id).await }
        });
        while !bridge.connected().await {
            tokio::task::yield_now().await;
        }
        poll
    }

    fn read(bridge: &Arc<Bridge>) -> tokio::task::JoinHandle<Result<Value, String>> {
        let bridge = bridge.clone();
        tokio::spawn(async move { bridge.call("browser_read".into(), json!({})).await })
    }

    #[tokio::test]
    async fn a_call_reaches_the_browser_and_is_answered_once() {
        let bridge = Arc::new(Bridge::new(AWAY));
        let poll = connect(&bridge, "one").await;
        let result = read(&bridge);
        let Next::Request(request) = poll.await.unwrap() else {
            panic!("the browser got no request");
        };
        assert_eq!(request.method, "browser_read");
        let wrong = answer(&request.id, json!({"wrong": true}));
        assert!(!bridge.reply("guess", wrong).await);
        let right = answer(&request.id, json!({"title": "Desk"}));
        assert!(bridge.reply(bridge.token(), right).await);
        let late = answer(&request.id, json!({}));
        assert!(!bridge.reply(bridge.token(), late).await);
        assert_eq!(result.await.unwrap().unwrap(), json!({"title": "Desk"}));
    }

    #[tokio::test]
    async fn only_this_runs_token_lets_a_browser_in() {
        let (bridge, earlier) = (Bridge::new(AWAY), Bridge::new(AWAY));
        let enter = bridge.enter(earlier.token(), "one", true).await;
        assert!(matches!(enter, Next::Refused));
        let next = bridge.next(earlier.token(), "one").await;
        assert!(matches!(next, Next::Refused));
        assert!(!bridge.connected().await);
    }

    #[tokio::test]
    async fn a_second_browser_waits_until_it_is_turned_to() {
        let bridge = Arc::new(Bridge::new(AWAY));
        let first = connect(&bridge, "one").await;
        let enter = bridge.enter(bridge.token(), "two", false).await;
        assert!(matches!(enter, Next::Busy));
        let next = bridge.next(bridge.token(), "two").await;
        assert!(matches!(next, Next::Busy));

        // What the first was asked is not left hanging.
        let asked = read(&bridge);
        assert!(matches!(first.await.unwrap(), Next::Request(_)));
        let taken = bridge.enter(bridge.token(), "two", true).await;
        assert!(matches!(taken, Next::Nothing));
        assert!(asked.await.unwrap().unwrap_err().contains("Another"));
        let next = bridge.next(bridge.token(), "one").await;
        assert!(matches!(next, Next::Busy));
    }

    #[tokio::test]
    async fn a_browser_that_quit_makes_room_and_is_not_called() {
        let bridge = Arc::new(Bridge::new(Duration::ZERO));
        let gone = bridge.browser("one", false).await.unwrap();
        assert!(!bridge.connected().await);
        let error = bridge.call("browser_read".into(), json!({})).await;
        assert!(error.unwrap_err().contains("not connected"));
        let second = bridge.browser("two", false).await.unwrap();
        assert_ne!(gone.id, second.id);
    }

    #[tokio::test]
    async fn a_browser_that_polls_again_is_the_same_one() {
        let bridge = Arc::new(Bridge::new(AWAY));
        let first = connect(&bridge, "one").await;
        let again = bridge.next(bridge.token(), "one");
        let waits = tokio::time::timeout(Duration::from_millis(100), again).await;
        assert!(waits.is_err(), "the second poll waits too");
        first.abort();
    }
}
