use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerSource {
    Mdns,
    Tailscale,
    Manual,
    /// Learned from another member's team list.
    Team,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerInfo {
    pub id: String,
    pub name: String,
    pub addresses: Vec<String>,
    pub online: bool,
    pub source: PeerSource,
    /// Seconds since this peer was last seen. The popover uses it for "Away".
    pub seen_secs_ago: u64,
    /// Automatic presence line: Headphones, On a call, In a meeting, Away, Online.
    pub status: String,
    pub avatar_hash: String,
    /// Color this machine uses for the peer.
    pub color: String,
    /// Letter or digit assigned on this Mac. Empty when unset.
    #[serde(default)]
    pub key: String,
}

#[derive(Debug, Clone)]
pub struct PeerRecord {
    pub id: String,
    pub name: String,
    pub addrs: Vec<SocketAddr>,
    pub source: PeerSource,
    pub last_seen: Instant,
    pub status: String,
    pub avatar_hash: String,
    /// A packet from this person, not only a mention in someone else's list.
    pub heard_directly: bool,
}

impl PeerRecord {
    pub fn to_info(&self, online: bool) -> PeerInfo {
        PeerInfo {
            id: self.id.clone(),
            name: self.name.clone(),
            addresses: self.addrs.iter().map(|a| a.to_string()).collect(),
            online,
            source: self.source,
            seen_secs_ago: self.last_seen.elapsed().as_secs(),
            status: self.status.clone(),
            avatar_hash: self.avatar_hash.clone(),
            color: String::new(),
            key: String::new(),
        }
    }
}

pub const PEER_STALE: std::time::Duration = std::time::Duration::from_secs(20);
