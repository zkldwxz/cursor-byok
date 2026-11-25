//! Relays MCP calls to the paired browser extension on this desktop.

use std::{collections::HashMap, sync::Arc, time::Duration};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::{mpsc, oneshot, Mutex};
use uuid::Uuid;

use crate::{store::Store, Result};

pub(crate) static BRIDGE: Bridge = Bridge::new();

pub(crate) struct Bridge {
    pair: Mutex<Option<Arc<Pair>>>,
}

struct Pair {
    token_hash: String,
    send: mpsc::Sender<BrowserRequest>,
    queue: Mutex<mpsc::Receiver<BrowserRequest>>,
    pending: Mutex<HashMap<String, oneshot::Sender<BrowserReply>>>,
}

impl Pair {
    fn new(token_hash: String) -> Arc<Self> {
        let (send, queue) = mpsc::channel(16);
        Arc::new(Self {
            token_hash,
            send,
            queue: Mutex::new(queue),
            pending: Mutex::new(HashMap::new()),
        })
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

impl Bridge {
    const fn new() -> Self {
        Self {
            pair: Mutex::const_new(None),
        }
    }

    /// Takes up the pairing of an earlier run.
    pub(crate) async fn restore(&self, store: &Store) -> Result<()> {
        if let Some(token_hash) = store.browser_pairing().await? {
            *self.pair.lock().await = Some(Pair::new(token_hash));
        }
        Ok(())
    }

    /// Replaces any earlier pairing. The token itself is kept by the extension alone.
    pub(crate) async fn pair(&self, store: &Store) -> Result<String> {
        let token = Uuid::new_v4().simple().to_string();
        let mut current = self.pair.lock().await;
        store.set_browser_pairing(&hash(&token)).await?;
        if let Some(previous) = current.replace(Pair::new(hash(&token))) {
            previous.pending.lock().await.clear();
        }
        Ok(token)
    }

    async fn authenticated(&self, token: &str) -> Option<Arc<Pair>> {
        self.pair
            .lock()
            .await
            .as_ref()
            .filter(|pair| pair.token_hash == hash(token))
            .cloned()
    }

    /// None when the token is not the paired one; otherwise the next request, if one came in time.
    pub(crate) async fn next(&self, token: &str) -> Option<Option<BrowserRequest>> {
        let pair = self.authenticated(token).await?;
        let request = tokio::time::timeout(Duration::from_secs(20), async {
            let mut queue = pair.queue.lock().await;
            while let Some(request) = queue.recv().await {
                if pair.pending.lock().await.contains_key(&request.id) {
                    return Some(request);
                }
            }
            None
        })
        .await
        .ok()
        .flatten();
        Some(request)
    }

    pub(crate) async fn reply(&self, token: &str, reply: BrowserReply) -> bool {
        let Some(pair) = self.authenticated(token).await else {
            return false;
        };
        let pending = pair.pending.lock().await.remove(&reply.id);
        pending.is_some_and(|sender| sender.send(reply).is_ok())
    }

    pub(crate) async fn call(
        &self,
        method: String,
        arguments: Value,
    ) -> std::result::Result<Value, String> {
        let pair = self.pair.lock().await.clone().ok_or(
            "Chrome is not paired; the user pairs its extension in BYOK Settings, Browser DevTools.",
        )?;
        let id = Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        let ttl = Duration::from_secs(45);
        pair.pending.lock().await.insert(id.clone(), tx);
        let request = BrowserRequest {
            id: id.clone(),
            method,
            arguments,
            expires_ms: super::runtime::now_ms() + ttl.as_millis() as u64,
        };
        if let Err(error) = pair.send.try_send(request) {
            pair.pending.lock().await.remove(&id);
            return Err(format!("Chrome bridge queue: {error}"));
        }
        let result = tokio::time::timeout(ttl, rx).await;
        pair.pending.lock().await.remove(&id);
        let reply = result
            .map_err(|_| {
                "Chrome did not answer; check that it is running with the paired extension."
            })?
            .map_err(|_| "Chrome pairing changed before the tool completed.")?;
        match (reply.result, reply.error) {
            (Some(result), None) => Ok(result),
            (None, Some(error)) => Err(error),
            _ => Err("Invalid Chrome bridge reply".into()),
        }
    }
}

fn hash(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    async fn store() -> (Store, String) {
        let directory = tempfile::tempdir().unwrap().keep();
        let url = format!("sqlite://{}", directory.join("state.db").display());
        (Store::connect(&url).await.unwrap(), url)
    }

    fn answer(id: &str, result: Value) -> BrowserReply {
        BrowserReply {
            id: id.into(),
            result: Some(result),
            error: None,
        }
    }

    #[tokio::test]
    async fn pairing_rotates_the_secret_and_completes_a_tool_once() {
        let bridge = Arc::new(Bridge::new());
        let (store, _) = store().await;
        let old = bridge.pair(&store).await.unwrap();
        let current = bridge.pair(&store).await.unwrap();
        assert!(bridge.next(&old).await.is_none());
        let result = tokio::spawn({
            let bridge = bridge.clone();
            async move {
                let arguments = json!({"action":"snapshot"});
                bridge.call("browser_read".into(), arguments).await
            }
        });
        let request = bridge.next(&current).await.unwrap().unwrap();
        assert_eq!(request.method, "browser_read");
        let wrong = answer(&request.id, json!({"wrong":true}));
        assert!(!bridge.reply(&old, wrong).await);
        let right = answer(&request.id, json!({"title":"Desk"}));
        assert!(bridge.reply(&current, right).await);
        let late = answer(&request.id, json!({}));
        assert!(!bridge.reply(&current, late).await);
        assert_eq!(result.await.unwrap().unwrap(), json!({"title":"Desk"}));
    }

    #[tokio::test]
    async fn a_pairing_outlives_byok_and_its_token_is_never_stored() {
        let (store, url) = store().await;
        let token = Bridge::new().pair(&store).await.unwrap();
        let kept = store.browser_pairing().await.unwrap().unwrap();
        assert!(!kept.contains(&token));

        let restarted = Bridge::new();
        assert!(restarted.authenticated(&token).await.is_none());
        let store = Store::connect(&url).await.unwrap();
        restarted.restore(&store).await.unwrap();
        assert!(restarted.authenticated(&token).await.is_some());
        assert!(restarted.authenticated(&kept).await.is_none());

        let newer = restarted.pair(&store).await.unwrap();
        let again = Bridge::new();
        again.restore(&store).await.unwrap();
        assert!(again.authenticated(&token).await.is_none());
        assert!(again.authenticated(&newer).await.is_some());
    }

    #[tokio::test]
    async fn a_pairing_that_cannot_be_kept_leaves_the_earlier_one() {
        let bridge = Bridge::new();
        let (store, _) = store().await;
        let token = bridge.pair(&store).await.unwrap();
        store.pool().close().await;
        assert!(bridge.pair(&store).await.is_err());
        assert!(bridge.authenticated(&token).await.is_some());
    }

    #[tokio::test]
    async fn an_extension_that_reconnects_is_not_taken_for_an_unpaired_one() {
        let bridge = Bridge::new();
        let (store, _) = store().await;
        let token = bridge.pair(&store).await.unwrap();
        tokio::select! {
            biased;
            _ = bridge.next(&token) => panic!("nothing was asked"),
            second = tokio::time::timeout(Duration::from_millis(100), bridge.next(&token)) => {
                assert!(second.is_err(), "the second poll waits too");
            }
        }
    }
}
