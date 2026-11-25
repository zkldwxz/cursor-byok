//! Issues one-time pairing codes and the device tokens they are exchanged for.
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const PAIRING_LIFETIME: Duration = Duration::from_secs(10 * 60);
const MAX_NAME_CHARS: usize = 40;

/// Everything here lives only while remote control is on: turning it off, or
/// quitting, forgets every device and ends its sessions.
pub(super) struct Access {
    pairing: Mutex<Option<Pairing>>,
    devices: Mutex<Vec<Device>>,
    /// Parent of every device's session.
    stop: CancellationToken,
}

struct Pairing {
    code: String,
    expires: Instant,
}

struct Device {
    view: PairedDevice,
    token_hash: [u8; 32],
    session: CancellationToken,
}

#[derive(Clone, Debug, Serialize)]
pub struct PairedDevice {
    pub id: String,
    pub name: String,
    pub paired_at_ms: i64,
}

impl Access {
    pub fn new(stop: CancellationToken) -> Self {
        Self {
            pairing: Mutex::default(),
            devices: Mutex::default(),
            stop,
        }
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

    pub fn pair(&self, code: &str, name: &str) -> Option<String> {
        {
            let mut pairing = self.pairing.lock();
            let current = pairing.as_ref()?;
            if hash(&current.code) != hash(code) || Instant::now() >= current.expires {
                return None;
            }
            *pairing = None;
        }
        let token = secret();
        self.devices.lock().push(Device {
            view: PairedDevice {
                id: Uuid::new_v4().to_string(),
                name: name.trim().chars().take(MAX_NAME_CHARS).collect(),
                paired_at_ms: chrono::Utc::now().timestamp_millis(),
            },
            token_hash: hash(&token),
            session: self.stop.child_token(),
        });
        Some(token)
    }

    /// The token's session: cancelled once its device is removed.
    pub fn session(&self, token: &str) -> Option<CancellationToken> {
        let token_hash = hash(token);
        self.devices
            .lock()
            .iter()
            .find(|device| device.token_hash == token_hash)
            .map(|device| device.session.clone())
    }

    pub fn remove(&self, id: &str) -> bool {
        let mut devices = self.devices.lock();
        let Some(index) = devices.iter().position(|device| device.view.id == id) else {
            return false;
        };
        devices.remove(index).session.cancel();
        true
    }

    pub fn devices(&self) -> Vec<PairedDevice> {
        self.devices
            .lock()
            .iter()
            .map(|device| device.view.clone())
            .collect()
    }

    #[cfg(test)]
    fn expire_pairing(&self) {
        self.pairing.lock().as_mut().unwrap().expires = Instant::now();
    }
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

    fn access() -> Access {
        Access::new(CancellationToken::new())
    }

    #[test]
    fn a_pairing_code_admits_exactly_one_device() {
        let access = access();
        let code = access.new_pairing_code();
        let token = access.pair(&code, "  phone  ").unwrap();
        assert!(access.session(&token).is_some());
        assert_eq!(access.devices()[0].name, "phone");
        assert_eq!(access.pair(&code, "second"), None);
    }

    #[test]
    fn a_wrong_guess_neither_pairs_nor_spoils_the_code() {
        let access = access();
        let code = access.new_pairing_code();
        assert_eq!(access.pair("guess", "x"), None);
        assert_eq!(access.pair("", "x"), None);
        assert!(access.pair(&code, "phone").is_some());
    }

    #[test]
    fn expired_and_replaced_codes_do_not_pair() {
        let access = access();
        let old = access.new_pairing_code();
        let code = access.new_pairing_code();
        assert_eq!(access.pair(&old, "phone"), None);
        access.expire_pairing();
        assert_eq!(access.pair(&code, "phone"), None);
    }

    #[test]
    fn removing_a_device_ends_its_session_and_only_its_own() {
        let access = access();
        let pair = || {
            access
                .pair(&access.new_pairing_code(), "phone")
                .map(|token| (access.session(&token).unwrap(), token))
                .unwrap()
        };
        let (first, first_token) = pair();
        let (second, _) = pair();
        assert!(access.remove(&access.devices()[0].id));
        assert!(first.is_cancelled());
        assert!(access.session(&first_token).is_none());
        assert!(!second.is_cancelled());
        assert!(!access.remove("missing"));
    }

    #[test]
    fn turning_remote_control_off_ends_every_session() {
        let stop = CancellationToken::new();
        let access = Access::new(stop.clone());
        let token = access.pair(&access.new_pairing_code(), "phone").unwrap();
        let session = access.session(&token).unwrap();
        stop.cancel();
        assert!(session.is_cancelled());
    }
}
