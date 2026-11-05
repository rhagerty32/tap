use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerSource {
    Mdns,
    Tailscale,
    Manual,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerInfo {
    pub id: String,
    pub name: String,
    pub addresses: Vec<String>,
    pub online: bool,
    pub source: PeerSource,
}

#[derive(Debug, Clone)]
pub struct PeerRecord {
    pub id: String,
    pub name: String,
    pub addrs: Vec<SocketAddr>,
    pub source: PeerSource,
    pub last_seen: Instant,
}

impl PeerRecord {
    pub fn to_info(&self, online: bool) -> PeerInfo {
        PeerInfo {
            id: self.id.clone(),
            name: self.name.clone(),
            addresses: self.addrs.iter().map(|a| a.to_string()).collect(),
            online,
            source: self.source,
        }
    }
}

pub const PEER_STALE: std::time::Duration = std::time::Duration::from_secs(20);
