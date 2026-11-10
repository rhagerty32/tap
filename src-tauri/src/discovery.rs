use crate::peer::{PeerRecord, PeerSource};
use crate::settings::{canonical_color, color_for_peer, Settings};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// `status` is normally well under a second. A stuck CLI must not freeze discovery.
const TAILSCALE_STATUS_TIMEOUT: Duration = Duration::from_secs(8);

/// `tailscale` is often missing from a menu-bar app's PATH. On macOS the
/// working binary ships inside Tailscale.app and is not installed on PATH.
fn tailscale_program() -> PathBuf {
    let mut candidates = Vec::new();
    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            candidates.push(dir.join("tailscale"));
            #[cfg(windows)]
            candidates.push(dir.join("tailscale.exe"));
        }
    }
    candidates.extend(tailscale_fallback_candidates());
    select_tailscale_program(candidates)
}

fn tailscale_fallback_candidates() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/Applications/Tailscale.app/Contents/MacOS/Tailscale"),
        PathBuf::from("/usr/local/bin/tailscale"),
        PathBuf::from("/opt/homebrew/bin/tailscale"),
        PathBuf::from("/usr/bin/tailscale"),
        #[cfg(windows)]
        PathBuf::from(r"C:\Program Files\Tailscale\tailscale.exe"),
    ]
}

fn select_tailscale_program(candidates: impl IntoIterator<Item = PathBuf>) -> PathBuf {
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from("tailscale"))
}

/// The macOS app binary opens its window unless this is set, and then
/// `status --json` never returns. Menu-bar apps have no terminal environment.
fn tailscale_status_command(program: &Path) -> Command {
    let mut command = Command::new(program);
    command
        .arg("status")
        .arg("--json")
        .env("TAILSCALE_BE_CLI", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    command
}

fn tailscale_status_stdout(program: &Path, timeout: Duration) -> Option<Vec<u8>> {
    let mut child = tailscale_status_command(program).spawn().ok()?;
    let Some(mut stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return None;
    };
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        let _ = tx.send(buf);
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => thread::sleep(Duration::from_millis(20)),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    if !status.success() {
        return None;
    }
    rx.recv_timeout(Duration::from_secs(2)).ok()
}

/// Online machines on this tailnet. They are not teammates until one answers
/// with the shared team key.
pub fn tailscale_targets(port: u16) -> Vec<SocketAddr> {
    let Some(stdout) = tailscale_status_stdout(&tailscale_program(), TAILSCALE_STATUS_TIMEOUT)
    else {
        return Vec::new();
    };
    tailscale_targets_from_status(&stdout, port)
}

pub fn tailscale_targets_from_status(json: &[u8], port: u16) -> Vec<SocketAddr> {
    let parsed: TailscaleStatus = match serde_json::from_slice(json) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let mut targets = Vec::new();
    let Some(peer_map) = parsed.peer else {
        return targets;
    };
    for peer in peer_map.into_values() {
        if peer.online != Some(true) {
            continue;
        }
        let Some(ip) = peer
            .tailscale_ips
            .as_deref()
            .and_then(preferred_tailscale_ip)
        else {
            continue;
        };
        let addr = SocketAddr::new(ip, port);
        if !targets.contains(&addr) {
            targets.push(addr);
        }
    }
    targets
}

fn preferred_tailscale_ip(ips: &[String]) -> Option<IpAddr> {
    let parsed: Vec<IpAddr> = ips.iter().filter_map(|ip| ip.parse().ok()).collect();
    parsed
        .iter()
        .copied()
        .find(IpAddr::is_ipv4)
        .or_else(|| parsed.into_iter().next())
}

pub fn dedupe_manual_peers(entries: &mut Vec<String>) {
    let mut seen = HashSet::new();
    entries.retain(|entry| {
        let key = entry.trim().to_ascii_lowercase();
        if key.is_empty() {
            return false;
        }
        seen.insert(key)
    });
}

/// Drop manual rows the user removed, then merge the ones they kept.
pub fn sync_manual_peers(map: &mut HashMap<String, PeerRecord>, manuals: Vec<PeerRecord>) {
    let ids: HashSet<&str> = manuals.iter().map(|peer| peer.id.as_str()).collect();
    map.retain(|_, peer| peer.source != PeerSource::Manual || ids.contains(peer.id.as_str()));
    let owned = std::mem::take(map);
    *map = merge_peer_maps(owned, manuals);
}

/// A typed host is an explicit "show this person", including after Remove.
pub fn reveal_manual_peers(settings: &mut Settings, map: &HashMap<String, PeerRecord>) -> bool {
    let manuals = manual_peer_records(&settings.manual_peers, settings.listen_port);
    if manuals.is_empty() {
        return false;
    }
    let before = settings.hidden_peers.len();
    settings.hidden_peers.retain(|id| {
        if manuals.iter().any(|manual| &manual.id == id) {
            return false;
        }
        match map.get(id) {
            Some(peer) => !manuals.iter().any(|manual| same_person(peer, manual)),
            None => true,
        }
    });
    before != settings.hidden_peers.len()
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

/// One row per person. The same Mac shows up as a display name, a manual host,
/// and a Tailscale hostname; those records stay separate in the map so each
/// path can refresh, and collapse here.
pub fn peer_list_from_map(
    map: &HashMap<String, PeerRecord>,
    settings: &Settings,
) -> Vec<crate::peer::PeerInfo> {
    let self_id = stable_sender_id(&settings.display_name);
    let peers: Vec<&PeerRecord> = map.values().filter(|p| p.id != self_id).collect();
    let mut infos: Vec<crate::peer::PeerInfo> = person_groups(&peers)
        .into_iter()
        .filter(|group| !group_hidden(&peers, group, settings))
        .map(|group| person_info(&peers, &group, settings))
        .collect();
    infos.sort_by(|a, b| {
        a.name
            .to_ascii_lowercase()
            .cmp(&b.name.to_ascii_lowercase())
            .then(a.id.cmp(&b.id))
    });
    infos
}

/// Every address for the person that `peer_id` belongs to, including paths
/// that are hidden behind the single row.
pub fn addresses_for_peer(map: &HashMap<String, PeerRecord>, peer_id: &str) -> Vec<SocketAddr> {
    let peers: Vec<&PeerRecord> = map.values().collect();
    let Some(start) = peers.iter().position(|p| p.id == peer_id) else {
        return Vec::new();
    };
    let Some(group) = person_groups(&peers)
        .into_iter()
        .find(|group| group.contains(&start))
    else {
        return Vec::new();
    };
    union_addrs(&peers, &group)
}

/// Hide this person on this Mac, including the other rows that are the same Mac.
pub fn hide_person(settings: &mut Settings, map: &HashMap<String, PeerRecord>, peer_id: &str) {
    let peers: Vec<&PeerRecord> = map.values().collect();
    let mut ids = Vec::new();
    if let Some(start) = peers.iter().position(|peer| peer.id == peer_id) {
        if let Some(group) = person_groups(&peers)
            .into_iter()
            .find(|group| group.contains(&start))
        {
            ids.extend(group.iter().map(|&index| peers[index].id.clone()));
        }
    }
    if !ids.iter().any(|id| id == peer_id) {
        ids.push(peer_id.to_string());
    }
    for id in &ids {
        if !settings.hidden_peers.iter().any(|existing| existing == id) {
            settings.hidden_peers.push(id.clone());
        }
        settings.peer_keys.remove(id);
    }
    let hidden: Vec<&PeerRecord> = peers
        .iter()
        .copied()
        .filter(|peer| ids.iter().any(|id| id == &peer.id))
        .collect();
    let port = settings.listen_port;
    settings.manual_peers.retain(|entry| {
        let Some(record) = parse_manual_peer(entry, port) else {
            return false;
        };
        !hidden.iter().any(|peer| same_person(peer, &record))
            && !ids.iter().any(|id| id == &record.id)
    });
}

pub fn sender_is_hidden(
    settings: &Settings,
    map: &HashMap<String, PeerRecord>,
    sender_id: &str,
    src_ip: IpAddr,
) -> bool {
    let manuals = manual_peer_records(&settings.manual_peers, settings.listen_port);
    if manuals
        .iter()
        .any(|manual| manual.addrs.iter().any(|addr| addr.ip() == src_ip) || manual.id == sender_id)
    {
        return false;
    }
    if settings.hidden_peers.iter().any(|id| id == sender_id) {
        return true;
    }
    let hidden: Vec<&PeerRecord> = map
        .values()
        .filter(|peer| settings.hidden_peers.iter().any(|id| id == &peer.id))
        .collect();
    if hidden
        .iter()
        .any(|peer| peer.addrs.iter().any(|addr| addr.ip() == src_ip))
    {
        return true;
    }
    map.get(sender_id)
        .is_some_and(|incoming| hidden.iter().any(|peer| same_person(peer, incoming)))
}

fn group_hidden(peers: &[&PeerRecord], group: &[usize], settings: &Settings) -> bool {
    let manuals = manual_peer_records(&settings.manual_peers, settings.listen_port);
    if group.iter().any(|&index| {
        manuals
            .iter()
            .any(|manual| manual.id == peers[index].id || same_person(manual, peers[index]))
    }) {
        return false;
    }
    group.iter().any(|&index| {
        settings
            .hidden_peers
            .iter()
            .any(|id| id == &peers[index].id)
    })
}

fn person_info(
    peers: &[&PeerRecord],
    group: &[usize],
    settings: &Settings,
) -> crate::peer::PeerInfo {
    let canonical = canonical_index(peers, group);
    let mut record = peers[canonical].clone();
    record.name = best_name(peers, group, canonical);
    record.addrs = union_addrs(peers, group);
    record.last_seen = group
        .iter()
        .map(|&i| peers[i].last_seen)
        .max()
        .unwrap_or(record.last_seen);
    record.heard_directly = group.iter().any(|&i| peers[i].heard_directly);
    record.status = group
        .iter()
        .map(|&i| peers[i])
        .filter(|p| !p.status.is_empty())
        .max_by_key(|p| p.last_seen)
        .map(|p| p.status.clone())
        .unwrap_or_default();
    record.avatar_hash = group
        .iter()
        .map(|&i| peers[i])
        .filter(|p| !p.avatar_hash.is_empty())
        .max_by_key(|p| (identity_rank(p), p.last_seen))
        .map(|p| p.avatar_hash.clone())
        .unwrap_or_default();
    let online = group.iter().any(|&i| record_online(peers[i]));
    let ids: Vec<&str> = group.iter().map(|&i| peers[i].id.as_str()).collect();
    let mut info = record.to_info(online);
    info.color = color_for_ids(settings, &ids, &info.id);
    info.key = key_for_ids(settings, &ids);
    info
}

fn person_groups(peers: &[&PeerRecord]) -> Vec<Vec<usize>> {
    let n = peers.len();
    let mut parent: Vec<usize> = (0..n).collect();
    for i in 0..n {
        for j in (i + 1)..n {
            if same_person(peers[i], peers[j]) {
                let ra = find(&mut parent, i);
                let rb = find(&mut parent, j);
                if ra != rb {
                    parent[rb] = ra;
                }
            }
        }
    }
    let mut buckets: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..n {
        buckets.entry(find(&mut parent, i)).or_default().push(i);
    }
    let mut groups: Vec<Vec<usize>> = buckets.into_values().collect();
    groups.sort_by_key(|group| group[0]);
    groups
}

fn find(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

fn same_person(a: &PeerRecord, b: &PeerRecord) -> bool {
    if a.addrs
        .iter()
        .any(|left| b.addrs.iter().any(|right| left.ip() == right.ip()))
    {
        return true;
    }
    let left = person_name(&a.name);
    let right = person_name(&b.name);
    !left.is_empty() && left == right
}

fn person_name(name: &str) -> String {
    name.trim().trim_end_matches('.').to_ascii_lowercase()
}

fn hostname_like(name: &str) -> bool {
    let trimmed = name.trim().trim_end_matches('.');
    if trimmed.is_empty() {
        return true;
    }
    let lower = trimmed.to_ascii_lowercase();
    lower.ends_with(".local") || lower.contains('.')
}

/// Prefer the app's own id over a typed host or a Tailscale node name.
fn identity_rank(peer: &PeerRecord) -> u8 {
    if peer.source == PeerSource::Manual || peer.id.starts_with("manual-") {
        0
    } else if peer.source == PeerSource::Tailscale || peer.id.starts_with("ts-") {
        1
    } else if peer.source == PeerSource::Team {
        2
    } else {
        3
    }
}

fn canonical_index(peers: &[&PeerRecord], group: &[usize]) -> usize {
    group
        .iter()
        .max_by_key(|&&i| {
            let peer = peers[i];
            (
                identity_rank(peer),
                !hostname_like(&peer.name),
                peer.heard_directly,
                peer.last_seen,
            )
        })
        .copied()
        .unwrap_or(group[0])
}

fn best_name(peers: &[&PeerRecord], group: &[usize], canonical: usize) -> String {
    let human = group
        .iter()
        .map(|&i| peers[i])
        .filter(|p| !hostname_like(&p.name) && !p.name.trim().is_empty())
        .max_by_key(|p| identity_rank(p));
    human
        .map(|p| p.name.clone())
        .unwrap_or_else(|| peers[canonical].name.clone())
}

fn union_addrs(peers: &[&PeerRecord], group: &[usize]) -> Vec<SocketAddr> {
    let mut addrs = Vec::new();
    for &i in group {
        for addr in &peers[i].addrs {
            if !addrs.contains(addr) {
                addrs.push(*addr);
            }
        }
    }
    addrs
}

fn record_online(peer: &PeerRecord) -> bool {
    match peer.source {
        PeerSource::Manual | PeerSource::Tailscale => true,
        // Introductions stay visible while the news is still spreading.
        PeerSource::Team => peer.last_seen.elapsed() < Duration::from_secs(70),
        PeerSource::Mdns => {
            let limit = if peer.heard_directly { 45 } else { 20 };
            peer.last_seen.elapsed() < Duration::from_secs(limit)
        }
    }
}

fn key_for_ids(settings: &Settings, ids: &[&str]) -> String {
    for id in ids {
        if let Some(key) = settings.peer_keys.get(*id) {
            if let Ok(normalized) = crate::settings::normalize_hotkey(key) {
                if !normalized.is_empty() {
                    return normalized;
                }
            }
        }
    }
    String::new()
}

/// Ids that belong to the same person as `peer_id`, including that id.
pub fn ids_for_person(map: &HashMap<String, PeerRecord>, peer_id: &str) -> Vec<String> {
    let peers: Vec<&PeerRecord> = map.values().collect();
    if let Some(start) = peers.iter().position(|peer| peer.id == peer_id) {
        if let Some(group) = person_groups(&peers)
            .into_iter()
            .find(|group| group.contains(&start))
        {
            return group.iter().map(|&index| peers[index].id.clone()).collect();
        }
    }
    vec![peer_id.to_string()]
}

fn color_for_ids(settings: &Settings, ids: &[&str], canonical: &str) -> String {
    for id in ids {
        if let Some(color) = settings.peer_colors.get(*id) {
            if let Some(hex) = canonical_color(color) {
                return hex.to_string();
            }
        }
    }
    color_for_peer(settings, canonical)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peer::{PeerRecord, PeerSource};
    use crate::settings::Settings;
    use std::collections::HashMap;
    use std::net::SocketAddr;
    use std::path::{Path, PathBuf};
    use std::time::Instant;

    #[test]
    fn manual_peer_keeps_explicit_port() {
        let peers = manual_peer_records(&["Alex@127.0.0.1:37562".into()], 37561);
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].name, "Alex");
        assert_eq!(peers[0].addrs[0].port(), 37562);
        assert_eq!(peers[0].addrs[0].ip().to_string(), "127.0.0.1");
    }

    #[test]
    fn tailnet_targets_are_online_ipv4_addresses() {
        let status = br#"{
            "Peer": {
                "dean": {
                    "HostName": "Deans-MacBook-Pro-2.local",
                    "Online": true,
                    "TailscaleIPs": ["fd7a:115c:a1e0::2", "100.64.0.2"]
                },
                "away": {
                    "HostName": "Alex",
                    "Online": false,
                    "TailscaleIPs": ["100.64.0.3"]
                }
            }
        }"#;
        let targets = tailscale_targets_from_status(status, 37561);
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].ip().to_string(), "100.64.0.2");
        assert_eq!(targets[0].port(), 37561);
    }

    fn record(id: &str, name: &str, ip: &str, source: PeerSource) -> PeerRecord {
        PeerRecord {
            id: id.into(),
            name: name.into(),
            addrs: vec![SocketAddr::new(ip.parse().unwrap(), 37561)],
            source,
            last_seen: Instant::now(),
            status: String::new(),
            avatar_hash: String::new(),
            heard_directly: source == PeerSource::Mdns,
        }
    }

    #[test]
    fn same_person_found_three_ways_is_one_row() {
        let mut map = HashMap::new();
        map.insert(
            "app-dean".into(),
            record("app-dean", "Dean", "100.64.0.2", PeerSource::Mdns),
        );
        map.insert(
            "manual-Dean-10.0.0.8".into(),
            record(
                "manual-Dean-10.0.0.8",
                "Dean",
                "10.0.0.8",
                PeerSource::Manual,
            ),
        );
        map.insert(
            "ts-dean".into(),
            record(
                "ts-dean",
                "Deans-MacBook-Pro-2.local",
                "100.64.0.2",
                PeerSource::Tailscale,
            ),
        );
        let list = peer_list_from_map(&map, &Settings::default());
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "app-dean");
        assert_eq!(list[0].name, "Dean");
        assert!(list[0].online);
        let ips: Vec<String> = list[0]
            .addresses
            .iter()
            .map(|addr| addr.split(':').next().unwrap().to_string())
            .collect();
        assert!(ips.contains(&"100.64.0.2".into()));
        assert!(ips.contains(&"10.0.0.8".into()));
        let hold = addresses_for_peer(&map, "app-dean");
        assert_eq!(hold.len(), 2);
    }

    #[test]
    fn different_people_stay_separate() {
        let mut map = HashMap::new();
        map.insert(
            "app-dean".into(),
            record("app-dean", "Dean", "100.64.0.2", PeerSource::Mdns),
        );
        map.insert(
            "app-alex".into(),
            record("app-alex", "Alex", "100.64.0.3", PeerSource::Mdns),
        );
        let list = peer_list_from_map(&map, &Settings::default());
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn remove_hides_every_path_for_that_person() {
        let mut map = HashMap::new();
        map.insert(
            "app-dean".into(),
            record("app-dean", "Dean", "100.64.0.2", PeerSource::Mdns),
        );
        map.insert(
            "manual-Dean-10.0.0.8".into(),
            record(
                "manual-Dean-10.0.0.8",
                "Dean",
                "10.0.0.8",
                PeerSource::Manual,
            ),
        );
        map.insert(
            "ts-dean".into(),
            record(
                "ts-dean",
                "Deans-MacBook-Pro-2.local",
                "100.64.0.2",
                PeerSource::Tailscale,
            ),
        );
        map.insert(
            "app-alex".into(),
            record("app-alex", "Alex", "100.64.0.3", PeerSource::Mdns),
        );
        let mut settings = Settings::default();
        settings.listen_port = 37561;
        settings.manual_peers.push("Dean@10.0.0.8".into());
        hide_person(&mut settings, &map, "app-dean");
        assert!(settings.hidden_peers.iter().any(|id| id == "ts-dean"));
        assert!(settings.manual_peers.is_empty());
        let list = peer_list_from_map(&map, &settings);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "Alex");
        assert!(sender_is_hidden(
            &settings,
            &map,
            "app-dean",
            "100.64.0.2".parse().unwrap(),
        ));
    }

    #[test]
    fn typed_host_shows_again_after_it_was_removed() {
        let mut map = HashMap::new();
        map.insert(
            "app-dean".into(),
            record("app-dean", "Dean", "100.113.108.22", PeerSource::Mdns),
        );
        let mut settings = Settings::default();
        settings.listen_port = 37561;
        settings.hidden_peers.push("app-dean".into());
        settings
            .hidden_peers
            .push("manual-Dean-100.113.108.22".into());
        settings.manual_peers = vec!["Dean@100.113.108.22".into(), "Dean@100.113.108.22".into()];
        dedupe_manual_peers(&mut settings.manual_peers);
        assert_eq!(settings.manual_peers.len(), 1);
        assert!(reveal_manual_peers(&mut settings, &map));
        assert!(settings.hidden_peers.is_empty());
        let manual = manual_peer_records(&settings.manual_peers, settings.listen_port);
        sync_manual_peers(&mut map, manual);
        let list = peer_list_from_map(&map, &settings);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "Dean");
        assert!(!sender_is_hidden(
            &settings,
            &map,
            "app-dean",
            "100.113.108.22".parse().unwrap(),
        ));
    }

    #[test]
    fn bare_ip_brings_back_the_removed_person() {
        let mut map = HashMap::new();
        map.insert(
            "app-dean".into(),
            record("app-dean", "Dean", "100.113.108.22", PeerSource::Mdns),
        );
        let mut settings = Settings::default();
        settings.listen_port = 37561;
        settings.hidden_peers.push("app-dean".into());
        settings.manual_peers = vec!["100.113.108.22".into()];
        assert!(reveal_manual_peers(&mut settings, &map));
        assert!(settings.hidden_peers.is_empty());
        let manual = manual_peer_records(&settings.manual_peers, settings.listen_port);
        assert_eq!(manual.len(), 1);
        assert_eq!(manual[0].id, "manual-100.113.108.22-100.113.108.22");
        sync_manual_peers(&mut map, manual);
        let list = peer_list_from_map(&map, &settings);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "app-dean");
        assert_eq!(list[0].name, "Dean");
    }

    #[test]
    fn typed_host_does_not_bring_back_someone_else() {
        let mut map = HashMap::new();
        map.insert(
            "app-dean".into(),
            record("app-dean", "Dean", "100.113.108.22", PeerSource::Mdns),
        );
        map.insert(
            "app-alex".into(),
            record("app-alex", "Alex", "100.64.0.3", PeerSource::Mdns),
        );
        let mut settings = Settings::default();
        settings.listen_port = 37561;
        settings.hidden_peers = vec!["app-dean".into(), "app-alex".into()];
        settings.manual_peers = vec!["Dean@100.113.108.22".into()];
        assert!(reveal_manual_peers(&mut settings, &map));
        assert_eq!(settings.hidden_peers, vec!["app-alex".to_string()]);
        let list = peer_list_from_map(&map, &settings);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "Dean");
    }

    #[test]
    fn sync_drops_a_manual_host_the_user_removed() {
        let mut map = HashMap::new();
        map.insert(
            "manual-Old-10.0.0.4".into(),
            record("manual-Old-10.0.0.4", "Old", "10.0.0.4", PeerSource::Manual),
        );
        map.insert(
            "app-alex".into(),
            record("app-alex", "Alex", "100.64.0.3", PeerSource::Mdns),
        );
        let manuals = manual_peer_records(&["Sam@10.0.0.5".into()], 37561);
        sync_manual_peers(&mut map, manuals);
        assert!(!map.contains_key("manual-Old-10.0.0.4"));
        assert!(map.contains_key("app-alex"));
        assert!(map.contains_key("manual-Sam-10.0.0.5"));
    }

    #[test]
    fn dedupe_manual_peers_folds_case_and_blanks() {
        let mut entries = vec![
            " Dean@10.0.0.8 ".into(),
            "dean@10.0.0.8".into(),
            "  ".into(),
            String::new(),
        ];
        dedupe_manual_peers(&mut entries);
        assert_eq!(entries, vec![" Dean@10.0.0.8 ".to_string()]);
    }

    #[test]
    fn tailnet_status_ignores_self_offline_and_duplicate_ips() {
        let status = br#"{
            "Self": {"Online": true, "TailscaleIPs": ["100.64.0.1"]},
            "Peer": {
                "me-echo": {"Online": true, "TailscaleIPs": ["100.64.0.2", "100.64.0.2"]},
                "same": {"Online": true, "TailscaleIPs": ["100.64.0.2"]},
                "idle": {"Online": false, "TailscaleIPs": ["100.64.0.4"]},
                "missing": {"TailscaleIPs": ["100.64.0.5"]},
                "v6": {"Online": true, "TailscaleIPs": ["fd7a:115c:a1e0::9"]}
            }
        }"#;
        let targets = tailscale_targets_from_status(status, 37561);
        let ips: Vec<_> = targets.iter().map(|addr| addr.ip().to_string()).collect();
        assert_eq!(
            ips.iter().filter(|ip| ip.as_str() == "100.64.0.2").count(),
            1
        );
        assert!(ips.contains(&"fd7a:115c:a1e0::9".to_string()));
        assert!(!ips.iter().any(|ip| ip == "100.64.0.1"));
        assert!(!ips.iter().any(|ip| ip == "100.64.0.4"));
        assert!(!ips.iter().any(|ip| ip == "100.64.0.5"));
        assert!(targets.iter().all(|addr| addr.port() == 37561));
    }

    #[test]
    fn tailnet_status_garbage_is_no_peers() {
        assert!(tailscale_targets_from_status(b"not-json", 37561).is_empty());
        assert!(tailscale_targets_from_status(b"{}", 37561).is_empty());
        assert!(tailscale_targets_from_status(br#"{"Peer":null}"#, 37561).is_empty());
    }

    #[test]
    fn tailscale_search_includes_paths_a_menu_bar_app_cannot_see() {
        let paths: Vec<_> = tailscale_fallback_candidates()
            .into_iter()
            .map(|path| path.display().to_string())
            .collect();
        assert!(paths
            .iter()
            .any(|path| path == "/Applications/Tailscale.app/Contents/MacOS/Tailscale"));
        assert!(paths.iter().any(|path| path == "/usr/local/bin/tailscale"));
        assert!(paths
            .iter()
            .any(|path| path == "/opt/homebrew/bin/tailscale"));
        assert!(paths.iter().any(|path| path == "/usr/bin/tailscale"));
    }

    #[test]
    fn tailscale_program_skips_missing_files_and_directories() {
        let scratch = Scratch::new("select");
        let missing = scratch.0.join("missing");
        let directory = scratch.0.join("dir");
        std::fs::create_dir(&directory).unwrap();
        let found = scratch.0.join("found");
        std::fs::write(&found, b"tailscale").unwrap();
        let selected =
            select_tailscale_program([missing.join("tailscale"), directory, found.clone()]);
        assert_eq!(selected, found);
        assert_eq!(
            select_tailscale_program([scratch.0.join("nope")]),
            PathBuf::from("tailscale")
        );
    }

    #[test]
    fn tailscale_status_forces_cli_mode() {
        use std::ffi::OsStr;
        let command = tailscale_status_command(Path::new("tailscale"));
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            [OsStr::new("status"), OsStr::new("--json")]
        );
        assert!(command
            .get_envs()
            .any(|(key, value)| { key == "TAILSCALE_BE_CLI" && value == Some(OsStr::new("1")) }));
    }

    #[cfg(unix)]
    #[test]
    fn menu_bar_tailscale_status_runs_as_the_cli() {
        use std::os::unix::fs::PermissionsExt;
        let scratch = Scratch::new("cli");
        let program = scratch.0.join("tailscale");
        std::fs::write(
            &program,
            "#!/bin/sh\nif [ \"$TAILSCALE_BE_CLI\" != \"1\" ]; then\n  exit 2\nfi\nprintf '%s' '{\"Peer\":{\"n\":{\"Online\":true,\"TailscaleIPs\":[\"100.64.0.9\"]}}}'\n",
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let stdout = tailscale_status_stdout(&program, Duration::from_secs(2)).expect("status");
        let targets = tailscale_targets_from_status(&stdout, 37561);
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].ip().to_string(), "100.64.0.9");
        assert_eq!(targets[0].port(), 37561);
    }

    #[cfg(unix)]
    #[test]
    fn failed_or_hung_tailscale_status_is_not_a_peer_list() {
        use std::os::unix::fs::PermissionsExt;
        let scratch = Scratch::new("hung");
        let failed = scratch.0.join("fail");
        std::fs::write(&failed, "#!/bin/sh\nexit 1\n").unwrap();
        std::fs::set_permissions(&failed, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(tailscale_status_stdout(&failed, Duration::from_secs(2)).is_none());

        let hung = scratch.0.join("hang");
        std::fs::write(&hung, "#!/bin/sh\nexec sleep 30\n").unwrap();
        std::fs::set_permissions(&hung, std::fs::Permissions::from_mode(0o755)).unwrap();
        let started = Instant::now();
        assert!(tailscale_status_stdout(&hung, Duration::from_millis(400)).is_none());
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("tap-discovery-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

#[derive(Debug, Deserialize)]
struct TailscaleStatus {
    #[serde(default, rename = "Peer", alias = "peer")]
    peer: Option<HashMap<String, TailscalePeer>>,
}

#[derive(Debug, Deserialize)]
struct TailscalePeer {
    #[serde(rename = "Online")]
    online: Option<bool>,
    #[serde(rename = "TailscaleIPs")]
    tailscale_ips: Option<Vec<String>>,
}
