use crate::peer::{PeerRecord, PeerSource};
use crate::settings::{color_for_peer, Settings};
use serde::Deserialize;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::process::Command;
use std::time::{Duration, Instant};

pub fn discover_tailscale_peers(port: u16) -> Vec<PeerRecord> {
    let output = Command::new("tailscale")
        .arg("status")
        .arg("--json")
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let parsed: TailscaleStatus = match serde_json::from_slice(&output.stdout) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };

    let mut peers = Vec::new();
    if let Some(peer_map) = parsed.peer {
        for (id, peer) in peer_map {
            if peer.online != Some(true) {
                continue;
            }
            let name = peer
                .dns_name
                .or(peer.host_name)
                .unwrap_or_else(|| id.clone());
            let mut addrs = Vec::new();
            if let Some(ip_str) = peer.tailscale_ips.and_then(|v| v.into_iter().next()) {
                if let Ok(ip) = ip_str.parse::<IpAddr>() {
                    addrs.push(SocketAddr::new(ip, port));
                }
            }
            if addrs.is_empty() {
                continue;
            }
            peers.push(PeerRecord {
                id: format!("ts-{id}"),
                name,
                addrs,
                source: PeerSource::Tailscale,
                last_seen: Instant::now(),
                status: String::new(),
                avatar_hash: String::new(),
                heard_directly: false,
            });
        }
    }
    peers
}

pub fn manual_peer_records(manual: &[String], port: u16) -> Vec<PeerRecord> {
    manual
        .iter()
        .filter_map(|entry| parse_manual_peer(entry, port))
        .collect()
}

fn parse_manual_peer(entry: &str, port: u16) -> Option<PeerRecord> {
    let entry = entry.trim();
    if entry.is_empty() {
        return None;
    }
    let (name, host) = if let Some((n, h)) = entry.split_once('@') {
        (n.trim(), h.trim())
    } else {
        (entry, entry)
    };
    let addr: SocketAddr = if host.contains(':') {
        host.parse().ok()?
    } else {
        format!("{host}:{port}").parse().ok()?
    };
    Some(PeerRecord {
        id: format!("manual-{name}-{host}"),
        name: name.to_string(),
        addrs: vec![addr],
        source: PeerSource::Manual,
        last_seen: Instant::now(),
        status: String::new(),
        avatar_hash: String::new(),
        heard_directly: true,
    })
}

fn preferred_source(current: PeerSource, incoming: PeerSource) -> PeerSource {
    fn rank(source: PeerSource) -> u8 {
        match source {
            PeerSource::Tailscale => 0,
            PeerSource::Team => 1,
            PeerSource::Mdns => 2,
            PeerSource::Manual => 3,
        }
    }
    if rank(incoming) >= rank(current) {
        incoming
    } else {
        current
    }
}

pub fn merge_peer_maps(
    mut base: HashMap<String, PeerRecord>,
    incoming: Vec<PeerRecord>,
) -> HashMap<String, PeerRecord> {
    for peer in incoming {
        base.entry(peer.id.clone())
            .and_modify(|existing| {
                existing.name = peer.name.clone();
                for addr in &peer.addrs {
                    if !existing.addrs.contains(addr) {
                        existing.addrs.push(*addr);
                    }
                }
                if peer.heard_directly {
                    existing.last_seen = Instant::now();
                    existing.heard_directly = true;
                }
                existing.source = preferred_source(existing.source, peer.source);
                if !peer.status.is_empty() {
                    existing.status = peer.status.clone();
                }
                if !peer.avatar_hash.is_empty() {
                    existing.avatar_hash = peer.avatar_hash.clone();
                }
            })
            .or_insert(peer);
    }
    base
}

use crate::network::stable_sender_id;

pub fn peer_list_from_map(
    map: &HashMap<String, PeerRecord>,
    settings: &Settings,
) -> Vec<crate::peer::PeerInfo> {
    let self_id = stable_sender_id(&settings.display_name);
    map.values()
        .filter(|p| p.id != self_id)
        .map(|p| {
            let online = match p.source {
                PeerSource::Manual | PeerSource::Tailscale => true,
                // Introductions stay visible while the news is still spreading.
                PeerSource::Team => p.last_seen.elapsed() < Duration::from_secs(70),
                PeerSource::Mdns => p.last_seen.elapsed() < Duration::from_secs(20),
            };
            let mut info = p.to_info(online);
            info.color = color_for_peer(settings, &p.id);
            info
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_peer_keeps_explicit_port() {
        let peers = manual_peer_records(&["Alex@127.0.0.1:37562".into()], 37561);
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].name, "Alex");
        assert_eq!(peers[0].addrs[0].port(), 37562);
        assert_eq!(peers[0].addrs[0].ip().to_string(), "127.0.0.1");
    }
}

#[derive(Debug, Deserialize)]
struct TailscaleStatus {
    #[serde(default)]
    peer: Option<HashMap<String, TailscalePeer>>,
}

#[derive(Debug, Deserialize)]
struct TailscalePeer {
    #[serde(rename = "HostName")]
    host_name: Option<String>,
    #[serde(rename = "DNSName")]
    dns_name: Option<String>,
    #[serde(rename = "Online")]
    online: Option<bool>,
    #[serde(rename = "TailscaleIPs")]
    tailscale_ips: Option<Vec<String>>,
}
