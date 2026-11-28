//! Issues one-time pairing codes and the lasting device tokens they are exchanged for.
use std::time::{Duration, Instant};

use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    store::{RemoteDevice, Store},
    Result,
};

const PAIRING_LIFETIME: Duration = Duration::from_secs(10 * 60);
const MAX_NAME_CHARS: usize = 40;

/// Who may come in while remote control is on. Phones stay paired until they
/// are removed; turning it off only ends their sessions.
pub(super) struct Access {
    store: Store,
    pairing: parking_lot::Mutex<Option<Pairing>>,
    /// Held while they are written, so that no two changes cross.
    devices: Mutex<Vec<Device>>,
    /// Parent of every device's session.
    stop: CancellationToken,
}

struct Pairing {
    code: String,
    expires: Instant,
}

struct Device {
    kept: RemoteDevice,
    session: CancellationToken,
}

#[derive(Clone, Debug, Serialize)]
pub struct PairedDevice {
    pub id: String,
    pub name: String,
    pub paired_at_ms: i64,
}

impl From<RemoteDevice> for PairedDevice {
    fn from(device: RemoteDevice) -> Self {
        Self {
            id: device.id,
            name: device.name,
            paired_at_ms: device.paired_at_ms,
        }
    }
}

impl Access {
    pub async fn open(store: Store, stop: CancellationToken) -> Result<Self> {
        let devices = store.remote_devices().await?.into_iter();
        let devices = devices.map(|kept| Device {
            kept,
            session: stop.child_token(),
        });
        Ok(Self {
            pairing: Default::default(),
            devices: Mutex::new(devices.collect()),
            store,
            stop,
        })
    }

    /// Replaces any earlier code, so only the newest link pairs.
    pub fn new_pairing_code(&self) -> String {
        let code = secret();
        *self.pairing.lock() = Some(Pairing {
            code: code.clone(),
            expires: Instant::now() + PAIRING_LIFETIME,
        });
        code
    }

    /// The device's token, if the code is the one of the newest link.
    /// A `device` the client keeps across pairings — its install id — pairs
    /// into the entry it already has rather than adding another one of the
    /// same phone, and the token it replaces stops working.
    pub async fn pair(
        &self,
        code: &str,
        name: &str,
        device: Option<&str>,
    ) -> Result<Option<String>> {
        {
            let mut pairing = self.pairing.lock();
            let Some(current) = pairing.as_ref() else {
                return Ok(None);
            };
            if hash(&current.code) != hash(code) || Instant::now() >= current.expires {
                return Ok(None);
            }
            *pairing = None;
        }
        let token = secret();
        let mut devices = self.devices.lock().await;
        let device = device.map(str::trim).filter(|device| !device.is_empty());
        // Devices kept without an install id — paired before there was one —
        // stay as they are; a re-pair then adds the id to its own entry only.
        let known = device.and_then(|device| {
            devices
                .iter()
                .position(|existing| existing.kept.device_id.as_deref() == Some(device))
        });
        let device = Device {
            kept: RemoteDevice {
                id: known
                    .map(|index| devices[index].kept.id.clone())
                    .unwrap_or_else(|| Uuid::new_v4().to_string()),
                name: name.trim().chars().take(MAX_NAME_CHARS).collect(),
                paired_at_ms: chrono::Utc::now().timestamp_millis(),
                token_hash: hash(&token),
                device_id: device.map(str::to_owned),
            },
            session: self.stop.child_token(),
        };
        match known {
            Some(index) => {
                let replaced = std::mem::replace(&mut devices[index], device);
                replaced.session.cancel();
                let mut kept = kept(&devices);
                kept[index] = devices[index].kept.clone();
                self.store.set_remote_devices(&kept).await?;
            }
            None => {
                let mut kept = kept(&devices);
                kept.push(device.kept.clone());
                self.store.set_remote_devices(&kept).await?;
                devices.push(device);
            }
        }
        Ok(Some(token))
    }

    /// The token's session: cancelled once its device is removed.
    pub async fn session(&self, token: &str) -> Option<CancellationToken> {
        let token_hash = hash(token);
        let devices = self.devices.lock().await;
        devices
            .iter()
            .find(|device| device.kept.token_hash == token_hash)
            .map(|device| device.session.clone())
    }

    pub async fn remove(&self, id: &str) -> Result<bool> {
        let mut devices = self.devices.lock().await;
        let Some(index) = devices.iter().position(|device| device.kept.id == id) else {
            return Ok(false);
        };
        let mut kept = kept(&devices);
        kept.remove(index);
        self.store.set_remote_devices(&kept).await?;
        devices.remove(index).session.cancel();
        Ok(true)
    }

    /// Remote control was turned off since.
    pub fn is_closed(&self) -> bool {
        self.stop.is_cancelled()
    }

    #[cfg(test)]
    fn expire_pairing(&self) {
        self.pairing.lock().as_mut().unwrap().expires = Instant::now();
    }
}

fn kept(devices: &[Device]) -> Vec<RemoteDevice> {
    devices.iter().map(|device| device.kept.clone()).collect()
}

fn secret() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

fn hash(value: &str) -> [u8; 32] {
    Sha256::digest(value.as_bytes()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn store() -> Store {
        let directory = tempfile::tempdir().unwrap().keep();
        let url = format!("sqlite://{}", directory.join("state.db").display());
        Store::connect(&url).await.unwrap()
    }

    async fn access() -> Access {
        let stop = CancellationToken::new();
        Access::open(store().await, stop).await.unwrap()
    }

    async fn paired(access: &Access, code: &str) -> Option<String> {
        access.pair(code, "  phone  ", None).await.unwrap()
    }

    async fn names(access: &Access) -> Vec<String> {
        let devices = access.store.remote_devices().await.unwrap();
        devices.into_iter().map(|device| device.name).collect()
    }

    #[tokio::test]
    async fn a_pairing_code_admits_exactly_one_device() {
        let access = access().await;
        let code = access.new_pairing_code();
        let token = paired(&access, &code).await.unwrap();
        assert!(access.session(&token).await.is_some());
        assert_eq!(names(&access).await, ["phone"]);
        assert_eq!(paired(&access, &code).await, None);
    }

    #[tokio::test]
    async fn a_wrong_guess_neither_pairs_nor_spoils_the_code() {
        let access = access().await;
        let code = access.new_pairing_code();
        assert_eq!(paired(&access, "guess").await, None);
        assert_eq!(paired(&access, "").await, None);
        assert!(paired(&access, &code).await.is_some());
    }

    #[tokio::test]
    async fn expired_and_replaced_codes_do_not_pair() {
        let access = access().await;
        let old = access.new_pairing_code();
        let code = access.new_pairing_code();
        assert_eq!(paired(&access, &old).await, None);
        access.expire_pairing();
        assert_eq!(paired(&access, &code).await, None);
    }

    #[tokio::test]
    async fn removing_a_device_ends_its_session_for_good_and_only_its_own() {
        let access = access().await;
        let mut tokens = Vec::new();
        for _ in 0..2 {
            let code = access.new_pairing_code();
            tokens.push(paired(&access, &code).await.unwrap());
        }
        let first = access.session(&tokens[0]).await.unwrap();
        let second = access.session(&tokens[1]).await.unwrap();
        let id = access.store.remote_devices().await.unwrap()[0].id.clone();
        assert!(access.remove(&id).await.unwrap());
        assert!(first.is_cancelled());
        assert!(access.session(&tokens[0]).await.is_none());
        assert!(!second.is_cancelled());
        assert!(!access.remove("missing").await.unwrap());
        let again = Access::open(access.store.clone(), CancellationToken::new());
        let again = again.await.unwrap();
        assert!(again.session(&tokens[0]).await.is_none());
        assert!(again.session(&tokens[1]).await.is_some());
    }

    #[tokio::test]
    async fn turning_remote_control_off_ends_every_session_and_keeps_every_device() {
        let store = store().await;
        let stop = CancellationToken::new();
        let access = Access::open(store.clone(), stop.clone()).await.unwrap();
        let code = access.new_pairing_code();
        let token = paired(&access, &code).await.unwrap();
        let session = access.session(&token).await.unwrap();
        stop.cancel();
        assert!(session.is_cancelled());
        let again = Access::open(store, CancellationToken::new()).await.unwrap();
        assert!(!again.session(&token).await.unwrap().is_cancelled());
    }

    #[tokio::test]
    async fn a_device_id_pairs_again_into_its_own_entry_not_a_new_one() {
        let access = access().await;
        let first = {
            let code = access.new_pairing_code();
            access
                .pair(&code, "phone", Some("install-1"))
                .await
                .unwrap()
                .unwrap()
        };
        let second = {
            let code = access.new_pairing_code();
            access
                .pair(&code, "phone", Some("install-1"))
                .await
                .unwrap()
                .unwrap()
        };
        let devices = access.store.remote_devices().await.unwrap();
        assert_eq!(devices.len(), 1);
        assert_ne!(first, second);
        // The token of before is retired: only the newest of one device enters.
        assert!(access.session(&first).await.is_none());
        assert!(access.session(&second).await.is_some());
    }

    #[tokio::test]
    async fn different_device_ids_stay_apart_and_a_missing_one_pairs_new() {
        let access = access().await;
        for device in [Some("install-1"), Some("install-2"), None] {
            let code = access.new_pairing_code();
            assert!(access.pair(&code, "phone", device).await.unwrap().is_some());
        }
        assert_eq!(access.store.remote_devices().await.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn no_token_itself_is_kept() {
        let access = access().await;
        let code = access.new_pairing_code();
        let token = paired(&access, &code).await.unwrap();
        let devices = access.store.remote_devices().await.unwrap();
        let written = serde_json::to_string(&devices).unwrap();
        assert!(!written.contains(&token));
    }
}
