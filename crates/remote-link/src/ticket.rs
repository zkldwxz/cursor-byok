//! The link a phone scans: where the computer is, its key, and a code that pairs once.
use std::net::SocketAddr;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use iroh::{EndpointId, RelayUrl};
use serde::{Deserialize, Serialize};

use crate::{Error, Result};

const PREFIX: &str = "cursor-byok://pair?d=";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Ticket {
    #[serde(rename = "k")]
    pub key: EndpointId,
    #[serde(rename = "c")]
    pub code: String,
    #[serde(rename = "a")]
    pub addresses: Vec<SocketAddr>,
    #[serde(rename = "r", skip_serializing_if = "Option::is_none", default)]
    pub relay: Option<Relay>,
}

/// A server that passes on what it cannot read.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Relay {
    #[serde(rename = "u")]
    pub address: String,
    #[serde(rename = "t", skip_serializing_if = "Option::is_none", default)]
    pub token: Option<String>,
}

impl Relay {
    pub(crate) fn url(&self) -> Result<RelayUrl> {
        self.address
            .parse()
            .map_err(|_| Error::Relay(self.address.clone()))
    }

    pub fn check(&self) -> Result<()> {
        self.url().map(drop)
    }
}

impl Ticket {
    pub fn link(&self) -> String {
        let json = serde_json::to_vec(self).expect("a ticket is plain data");
        format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(json))
    }

    pub fn parse(link: &str) -> Result<Self> {
        let invalid = |error: &dyn std::fmt::Display| Error::Ticket(error.to_string());
        let data = link
            .trim()
            .strip_prefix(PREFIX)
            .ok_or_else(|| invalid(&"it is not a Cursor BYOK link"))?;
        let json = URL_SAFE_NO_PAD.decode(data).map_err(|it| invalid(&it))?;
        serde_json::from_slice(&json).map_err(|it| invalid(&it))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ticket(relay: Option<Relay>) -> Ticket {
        Ticket {
            key: iroh::SecretKey::generate().public(),
            code: "once".into(),
            addresses: vec!["192.168.1.2:4000".parse().unwrap()],
            relay,
        }
    }

    #[test]
    fn a_link_carries_everything_a_phone_needs() {
        for relay in [
            None,
            Some(Relay {
                address: "https://relay.example".into(),
                token: Some("secret".into()),
            }),
        ] {
            let ticket = ticket(relay);
            assert_eq!(Ticket::parse(&ticket.link()).unwrap(), ticket);
        }
    }

    #[test]
    fn anything_else_is_refused() {
        for link in [
            "",
            "https://example.com/#code",
            "cursor-byok://pair?d=!!",
            "cursor-byok://pair?d=e30",
        ] {
            assert!(Ticket::parse(link).is_err(), "{link}");
        }
    }

    #[test]
    fn a_relay_address_that_is_no_address_is_named() {
        let relay = Relay {
            address: "not an address".into(),
            token: None,
        };
        assert!(relay
            .check()
            .unwrap_err()
            .to_string()
            .contains("not an address"));
    }
}
