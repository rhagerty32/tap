use crate::avatar;
use crate::discovery::{
    addresses_for_peer, manual_peer_records, merge_peer_maps, peer_list_from_map,
    reveal_manual_peers, sender_is_hidden, sync_manual_peers, tailscale_targets,
};
use crate::focus;
use crate::overlay::sync_overlay_windows;
use crate::overlay::OverlayChrome;
use crate::overlay_state::{OverlaySnapshot, OverlayStateMachine};
use crate::peer::{PeerRecord, PeerSource};
use crate::protocol::{
    new_outgoing, sign_message, verify_message, MessageKind, ReplayGuard, RosterMember, TapMessage,
};
use crate::settings::{color_for_peer, effective_dnd, is_tailnet_ip, team_key_bytes, Settings};
use crate::status;
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use parking_lot::Mutex;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::net::{IpAddr, SocketAddr, TcpListener, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::{Condvar, Mutex as StdMutex};
use std::thread;
use std::time::SystemTime;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

const SERVICE_TYPE: &str = "_tap._udp.local.";
/// How long a person we heard ourselves stays in the list we send.
const ROSTER_DIRECT_TTL: Duration = Duration::from_secs(5 * 60);
/// How long a person we only heard about keeps being passed on.
const ROSTER_INTRO_TTL: Duration = Duration::from_secs(60);
/// How long a departure keeps being announced.
const ROSTER_TOMBSTONE_TTL: Duration = Duration::from_secs(10 * 60);
const ROSTER_REFRESH: Duration = Duration::from_secs(15);
const ROSTER_MAX_MEMBERS: usize = 32;

struct RosterSync {
    gone: HashMap<String, Instant>,
    published: String,
    last_forced: Instant,
}

impl Default for RosterSync {
    fn default() -> Self {
        Self {
            gone: HashMap::new(),
            published: String::new(),
            last_forced: Instant::now() - ROSTER_REFRESH,
        }
    }
}
/// 2 means "not yet published". 0 and 1 are the last effective do-not-disturb bit.
static LAST_DND: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(2);

pub struct NetworkRuntime {
    stop: Arc<AtomicBool>,
    hold_target: Arc<Mutex<Option<String>>>,
    hold_addrs: Arc<Mutex<Vec<SocketAddr>>>,
    settings: Arc<Mutex<Settings>>,
    peers: Arc<Mutex<HashMap<String, PeerRecord>>>,
    roster: Arc<Mutex<RosterSync>>,
    discovery: Arc<Discovery>,
}

impl NetworkRuntime {
    pub fn start(
        app: AppHandle,
        settings: Arc<Mutex<Settings>>,
        overlay: Arc<Mutex<OverlayStateMachine>>,
        peers: Arc<Mutex<HashMap<String, PeerRecord>>>,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let hold_target = Arc::new(Mutex::new(None));
        let hold_addrs = Arc::new(Mutex::new(Vec::new()));
        let replay: Arc<Mutex<ReplayGuard>> = Arc::new(Mutex::new(ReplayGuard::default()));
        let roster = Arc::new(Mutex::new(RosterSync::default()));
        let discovery = Arc::new(Discovery::new());

        spawn_receiver(
            app.clone(),
            settings.clone(),
            overlay.clone(),
            peers.clone(),
            replay.clone(),
            roster.clone(),
            stop.clone(),
        );
        spawn_presence(
            app.clone(),
            settings.clone(),
            peers.clone(),
            roster.clone(),
            discovery.clone(),
            stop.clone(),
        );
        spawn_hold_sender(
            settings.clone(),
            hold_target.clone(),
            hold_addrs.clone(),
            stop.clone(),
        );
        spawn_mdns(
            app.clone(),
            settings.clone(),
            peers.clone(),
            roster.clone(),
            stop.clone(),
        );
        spawn_tailscale_poll(
            app.clone(),
            settings.clone(),
            peers.clone(),
            roster.clone(),
            stop.clone(),
        );
        spawn_avatar_server(settings.clone(), stop.clone());

        Self {
            stop,
            hold_target,
            hold_addrs,
            settings,
            peers,
            roster,
            discovery,
        }
    }

    pub fn request_refresh(&self) {
        let mut force = self.discovery.force.lock().expect("discovery lock");
        *force = true;
        self.discovery.wake.notify_one();
    }

    pub fn refreshed_at(&self) -> Option<u64> {
        *self.discovery.refreshed_at.lock().expect("discovery lock")
    }

    pub fn stop(&self) {
        send_goodbye(&self.settings, &self.peers, &self.roster);
        self.stop.store(true, Ordering::SeqCst);
        self.discovery.wake.notify_one();
    }

    pub fn begin_hold(&self, peer_id: &str, addrs: Vec<SocketAddr>) {
        *self.hold_target.lock() = Some(peer_id.to_string());
        *self.hold_addrs.lock() = addrs;
    }

    pub fn end_hold(&self) {
        // Leave the addresses in place. The hold thread sends Stop on the next
        // wake, then clears them. Clearing here raced that wake and dropped Stop.
        *self.hold_target.lock() = None;
    }
}

fn spawn_receiver(
    app: AppHandle,
    settings: Arc<Mutex<Settings>>,
    overlay: Arc<Mutex<OverlayStateMachine>>,
    peers: Arc<Mutex<HashMap<String, PeerRecord>>>,
    replay: Arc<Mutex<ReplayGuard>>,
    roster: Arc<Mutex<RosterSync>>,
    stop: Arc<AtomicBool>,
) {
    thread::spawn(move || {
        let socket = loop {
            if stop.load(Ordering::SeqCst) {
                return;
            }
            let port = settings.lock().listen_port;
            match UdpSocket::bind(format!("0.0.0.0:{port}")) {
                Ok(s) => break s,
                Err(_) => thread::sleep(Duration::from_millis(500)),
            }
        };
        let _ = socket.set_nonblocking(true);

        let mut buf = [0u8; 8192];
        let mut last_tick = Instant::now();
        while !stop.load(Ordering::SeqCst) {
            loop {
                match socket.recv_from(&mut buf) {
                    Ok((len, src)) => match serde_json::from_slice::<TapMessage>(&buf[..len]) {
                        Ok(msg) => {
                            handle_inbound(
                                &app,
                                &settings,
                                &overlay,
                                &peers,
                                &replay,
                                &roster,
                                msg,
                                src.ip(),
                            );
                        }
                        Err(_) => {}
                    },
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(_) => break,
                }
            }

            if last_tick.elapsed() >= Duration::from_millis(200) {
                last_tick = Instant::now();
                // Heartbeats only refresh the timeout; they do not emit.
                // Re-publishing the same snapshot here used to call
                // apply_overlay_windows several times a second, which re-pinned
                // every window and logged "glow on N display(s)" for the whole tap.
                let changed = {
                    let mut o = overlay.lock();
                    let before = o.snapshot();
                    o.tick(Instant::now());
                    let after = o.snapshot();
                    if before == after {
                        None
                    } else {
                        Some(after)
                    }
                };
                if let Some(changed) = changed {
                    emit_overlay(&app, &settings, changed);
                }
            }
            thread::sleep(Duration::from_millis(20));
        }
    });
}

#[derive(Debug)]
enum InboundEffect {
    Ignored,
    PeersChanged,
    OverlayChanged(OverlaySnapshot),
    Busy(IpAddr),
    PeerBusy(String),
}

fn apply_inbound(
    cfg: &Settings,
    overlay: &mut OverlayStateMachine,
    peers: &mut HashMap<String, PeerRecord>,
    replay: &mut ReplayGuard,
    roster: &mut RosterSync,
    msg: TapMessage,
    src_ip: IpAddr,
    app: Option<&AppHandle>,
) -> InboundEffect {
    if cfg.tailnet_only && !is_tailnet_ip(&src_ip) {
        return InboundEffect::Ignored;
    }
    let Some(key) = team_key_bytes(cfg) else {
        return InboundEffect::Ignored;
    };
    if verify_message(&key, &msg, replay).is_err() {
        return InboundEffect::Ignored;
    }

    if cfg
        .mute_senders
        .get(&msg.sender_id)
        .copied()
        .unwrap_or(false)
        || sender_is_hidden(cfg, peers, &msg.sender_id, src_ip)
    {
        return InboundEffect::Ignored;
    }
    if cfg.allow_senders.values().any(|v| *v)
        && !cfg
            .allow_senders
            .get(&msg.sender_id)
            .copied()
            .unwrap_or(false)
    {
        return InboundEffect::Ignored;
    }
    if effective_dnd(cfg, focus::cached_system_focus())
        && matches!(msg.kind, MessageKind::Start | MessageKind::Heartbeat)
    {
        return if matches!(msg.kind, MessageKind::Start) {
            InboundEffect::Busy(src_ip)
        } else {
            InboundEffect::Ignored
        };
    }

    match msg.kind {
        MessageKind::Presence => {
            let sender_id = msg.sender_id.clone();
            let avatar_hash = msg.avatar_hash.clone();
            *peers = merge_peer_maps(
                peers.clone(),
                vec![PeerRecord {
                    id: sender_id.clone(),
                    name: msg.sender_name.clone(),
                    addrs: vec![SocketAddr::new(src_ip, cfg.listen_port)],
                    source: PeerSource::Mdns,
                    last_seen: Instant::now(),
                    status: msg.status.clone(),
                    avatar_hash: avatar_hash.clone(),
                    heard_directly: true,
                }],
            );
            roster.gone.remove(&sender_id);
            InboundEffect::PeersChanged
        }
        MessageKind::Busy => InboundEffect::PeerBusy(msg.sender_id),
        MessageKind::Start => {
            let color = color_for_peer(cfg, &msg.sender_id);
            overlay.apply_start(msg.sender_id, msg.sender_name, color);
            InboundEffect::OverlayChanged(overlay.snapshot())
        }
        MessageKind::Heartbeat => {
            overlay.apply_heartbeat(&msg.sender_id);
            InboundEffect::Ignored
        }
        MessageKind::Stop => {
            overlay.apply_stop(&msg.sender_id);
            InboundEffect::OverlayChanged(overlay.snapshot())
        }
        MessageKind::Roster => apply_roster(cfg, peers, roster, &msg, src_ip),
    }
}

fn apply_roster(
    cfg: &Settings,
    peers: &mut HashMap<String, PeerRecord>,
    roster: &mut RosterSync,
    msg: &TapMessage,
    src_ip: IpAddr,
) -> InboundEffect {
    if msg.members.len() > ROSTER_MAX_MEMBERS {
        return InboundEffect::Ignored;
    }
    let self_id = stable_sender_id(&cfg.display_name);
    let mut changed = false;
    let sender_left = msg
        .members
        .iter()
        .any(|member| member.id == msg.sender_id && member.gone);
    if sender_left {
        if msg.sender_id != self_id {
            changed |= peers.remove(&msg.sender_id).is_some();
            changed |= remember_gone(roster, &msg.sender_id);
        }
    } else {
        changed |= admit_direct(
            peers,
            roster,
            &msg.sender_id,
            &msg.sender_name,
            SocketAddr::new(src_ip, cfg.listen_port),
        );
    }

    for member in &msg.members {
        if member.id.is_empty() || member.id == self_id {
            continue;
        }
        if member.gone {
            changed |= peers.remove(&member.id).is_some();
            changed |= remember_gone(roster, &member.id);
            continue;
        }
        if member.id == msg.sender_id || roster.gone.contains_key(&member.id) {
            continue;
        }
        changed |= admit_introduced(peers, member, cfg.listen_port);
    }
    if changed {
        InboundEffect::PeersChanged
    } else {
        InboundEffect::Ignored
    }
}

fn remember_gone(roster: &mut RosterSync, id: &str) -> bool {
    if id.is_empty() || roster.gone.contains_key(id) {
        return false;
    }
    roster.gone.insert(id.to_string(), Instant::now());
    true
}

fn admit_direct(
    peers: &mut HashMap<String, PeerRecord>,
    roster: &mut RosterSync,
    id: &str,
    name: &str,
    addr: SocketAddr,
) -> bool {
    if id.is_empty() {
        return false;
    }
    let revived = roster.gone.remove(id).is_some();
    let mut incoming = vec![PeerRecord {
        id: id.to_string(),
        name: name.to_string(),
        addrs: vec![addr],
        source: PeerSource::Mdns,
        last_seen: Instant::now(),
        status: String::new(),
        avatar_hash: String::new(),
        heard_directly: true,
    }];
    let new_addr = peers
        .get(id)
        .map(|peer| !peer.addrs.contains(&addr))
        .unwrap_or(true);
    let is_new = !peers.contains_key(id);
    *peers = merge_peer_maps(std::mem::take(peers), std::mem::take(&mut incoming));
    revived || is_new || new_addr
}

fn admit_introduced(
    peers: &mut HashMap<String, PeerRecord>,
    member: &RosterMember,
    port: u16,
) -> bool {
    let addrs: Vec<SocketAddr> = member
        .addrs
        .iter()
        .filter_map(|value| parse_member_addr(value, port))
        .take(4)
        .collect();
    if addrs.is_empty() {
        return false;
    }
    if let Some(existing) = peers.get_mut(&member.id) {
        let mut added = false;
        if existing.name != member.name && !member.name.is_empty() {
            existing.name = member.name.clone();
            added = true;
        }
        for addr in addrs {
            if !existing.addrs.contains(&addr) {
                existing.addrs.push(addr);
                added = true;
            }
        }
        return added;
    }
    peers.insert(
        member.id.clone(),
        PeerRecord {
            id: member.id.clone(),
            name: member.name.clone(),
            addrs,
            source: PeerSource::Team,
            last_seen: Instant::now(),
            status: String::new(),
            avatar_hash: String::new(),
            heard_directly: false,
        },
    );
    true
}

fn parse_member_addr(value: &str, port: u16) -> Option<SocketAddr> {
    if let Ok(addr) = value.parse() {
        return Some(addr);
    }
    let ip: IpAddr = value.parse().ok()?;
    Some(SocketAddr::new(ip, port))
}

fn should_announce(peer: &PeerRecord) -> bool {
    match peer.source {
        PeerSource::Tailscale => false,
        PeerSource::Manual => true,
        PeerSource::Mdns | PeerSource::Team => {
            let ttl = if peer.heard_directly {
                ROSTER_DIRECT_TTL
            } else {
                ROSTER_INTRO_TTL
            };
            peer.last_seen.elapsed() < ttl
        }
    }
}

fn local_tap_addrs(port: u16) -> Vec<String> {
    let mut addrs = Vec::new();
    if let Ok(ifaces) = if_addrs::get_if_addrs() {
        for iface in ifaces {
            let if_addrs::IfAddr::V4(v4) = iface.addr else {
                continue;
            };
            if v4.ip.is_loopback() || !v4.ip.is_private() {
                continue;
            }
            let addr = SocketAddr::new(IpAddr::V4(v4.ip), port).to_string();
            if !addrs.contains(&addr) {
                addrs.push(addr);
            }
            if addrs.len() == 4 {
                break;
            }
        }
    }
    addrs
}

fn roster_members(
    cfg: &Settings,
    peers: &HashMap<String, PeerRecord>,
    gone: &HashMap<String, Instant>,
) -> Vec<RosterMember> {
    let self_id = stable_sender_id(&cfg.display_name);
    let mut members = vec![RosterMember {
        id: self_id.clone(),
        name: cfg.display_name.clone(),
        addrs: local_tap_addrs(cfg.listen_port),
        gone: false,
    }];
    let mut present: Vec<_> = peers
        .values()
        .filter(|peer| peer.id != self_id && should_announce(peer))
        .cloned()
        .collect();
    present.sort_by(|a, b| a.id.cmp(&b.id));
    for peer in present {
        if members.len() >= ROSTER_MAX_MEMBERS {
            break;
        }
        members.push(RosterMember {
            id: peer.id,
            name: peer.name,
            addrs: peer
                .addrs
                .iter()
                .take(4)
                .map(|addr| addr.to_string())
                .collect(),
            gone: false,
        });
    }
    let mut left: Vec<_> = gone
        .iter()
        .filter(|(_, at)| at.elapsed() < ROSTER_TOMBSTONE_TTL)
        .map(|(id, _)| id.clone())
        .collect();
    left.sort();
    for id in left {
        if members.iter().any(|member| member.id == id) || members.len() >= ROSTER_MAX_MEMBERS {
            continue;
        }
        members.push(RosterMember {
            id,
            name: String::new(),
            addrs: Vec::new(),
            gone: true,
        });
    }
    members
}

fn send_members(cfg: &Settings, peers: &HashMap<String, PeerRecord>, members: &[RosterMember]) {
    let Some(key) = team_key_bytes(cfg) else {
        return;
    };
    let self_id = stable_sender_id(&cfg.display_name);
    let mut msg = new_outgoing(MessageKind::Roster, &self_id, &cfg.display_name, "", "");
    msg.members = members.to_vec();
    sign_message(&key, &mut msg);
    let Ok(data) = serde_json::to_vec(&msg) else {
        return;
    };
    let Ok(socket) = UdpSocket::bind("0.0.0.0:0") else {
        return;
    };
    let mut targets = HashSet::new();
    for peer in peers.values() {
        if peer.id == self_id {
            continue;
        }
        for addr in &peer.addrs {
            targets.insert(*addr);
        }
    }
    for addr in targets {
        let _ = socket.send_to(&data, addr);
    }
}

fn maybe_publish_roster(
    cfg: &Settings,
    peers: &Mutex<HashMap<String, PeerRecord>>,
    roster: &Mutex<RosterSync>,
    force: bool,
) {
    let peer_map = peers.lock().clone();
    let members = {
        let mut roster = roster.lock();
        roster
            .gone
            .retain(|_, at| at.elapsed() < ROSTER_TOMBSTONE_TTL);
        let members = roster_members(cfg, &peer_map, &roster.gone);
        let signature = serde_json::to_string(&members).unwrap_or_default();
        if !force && signature == roster.published {
            return;
        }
        roster.published = signature;
        members
    };
    send_members(cfg, &peer_map, &members);
}

fn send_goodbye(
    settings: &Mutex<Settings>,
    peers: &Mutex<HashMap<String, PeerRecord>>,
    roster: &Mutex<RosterSync>,
) {
    let cfg = settings.lock().clone();
    let peer_map = peers.lock().clone();
    let gone = roster.lock().gone.clone();
    let self_id = stable_sender_id(&cfg.display_name);
    let mut members = roster_members(&cfg, &peer_map, &gone);
    if let Some(myself) = members.iter_mut().find(|member| member.id == self_id) {
        myself.gone = true;
        myself.addrs.clear();
    }
    send_members(&cfg, &peer_map, &members);
}

fn handle_inbound(
    app: &AppHandle,
    settings: &Arc<Mutex<Settings>>,
    overlay: &Arc<Mutex<OverlayStateMachine>>,
    peers: &Arc<Mutex<HashMap<String, PeerRecord>>>,
    replay: &Arc<Mutex<ReplayGuard>>,
    roster: &Arc<Mutex<RosterSync>>,
    msg: TapMessage,
    src_ip: IpAddr,
) {
    let cfg = settings.lock().clone();
    let effect = {
        let mut overlay_guard = overlay.lock();
        let mut peers_guard = peers.lock();
        let mut replay_guard = replay.lock();
        let mut roster_guard = roster.lock();
        let kind = msg.kind;
        let sender_id = msg.sender_id.clone();
        let avatar_hash = msg.avatar_hash.clone();
        let effect = apply_inbound(
            &cfg,
            &mut overlay_guard,
            &mut peers_guard,
            &mut replay_guard,
            &mut roster_guard,
            msg,
            src_ip,
            Some(app),
        );
        if matches!(kind, MessageKind::Presence | MessageKind::Start) {
            maybe_fetch_avatar(
                app,
                settings,
                overlay,
                &cfg,
                &sender_id,
                &avatar_hash,
                src_ip,
            );
        }
        effect
    };
    match effect {
        InboundEffect::Ignored => {}
        InboundEffect::PeersChanged => {
            let list = peer_list_from_map(&peers.lock(), &cfg);
            let _ = app.emit("peers-updated", list);
            maybe_publish_roster(&cfg, peers, roster, false);
        }
        InboundEffect::OverlayChanged(snapshot) => emit_overlay(app, settings, snapshot),
        InboundEffect::Busy(ip) => send_busy(settings, ip),
        InboundEffect::PeerBusy(id) => {
            let _ = app.emit("peer-busy", id);
        }
    }
}

fn maybe_fetch_avatar(
    app: &AppHandle,
    settings: &Arc<Mutex<Settings>>,
    overlay: &Arc<Mutex<OverlayStateMachine>>,
    cfg: &Settings,
    id: &str,
    hash: &str,
    src_ip: IpAddr,
) {
    if hash.is_empty() || avatar::has_cache(id, hash) {
        return;
    }
    if id == stable_sender_id(&cfg.display_name) {
        return;
    }
    let Some(key) = team_key_bytes(cfg) else {
        return;
    };
    let addr = SocketAddr::new(src_ip, cfg.listen_port);
    let id = id.to_string();
    let hash = hash.to_string();
    let app = app.clone();
    let settings = settings.clone();
    let overlay = overlay.clone();
    thread::spawn(move || {
        let Some(bytes) = avatar::fetch(addr, &key) else {
            return;
        };
        if avatar::hash_of(&bytes) != hash {
            return;
        }
        if avatar::write_cache(&id, &hash, &bytes).is_err() {
            return;
        }
        let _ = app.emit(
            "avatar-updated",
            serde_json::json!({ "id": id, "hash": hash }),
        );
        let snapshot = {
            let overlay = overlay.lock();
            match overlay.snapshot() {
                OverlaySnapshot::Visible(active) if active.sender_id == id => {
                    Some(overlay.snapshot())
                }
                _ => None,
            }
        };
        if let Some(snapshot) = snapshot {
            emit_overlay(&app, &settings, snapshot);
        }
    });
}

fn send_busy(settings: &Arc<Mutex<Settings>>, ip: IpAddr) {
    let cfg = settings.lock().clone();
    let Some(key) = team_key_bytes(&cfg) else {
        return;
    };
    let port = cfg.listen_port;
    let id = stable_sender_id(&cfg.display_name);
    let name = cfg.display_name.clone();
    thread::spawn(move || {
        let Ok(socket) = UdpSocket::bind("0.0.0.0:0") else {
            return;
        };
        let mut msg = new_outgoing(MessageKind::Busy, &id, &name, "", "");
        sign_message(&key, &mut msg);
        if let Ok(bytes) = serde_json::to_vec(&msg) {
            let _ = socket.send_to(&bytes, SocketAddr::new(ip, port));
        }
    });
}

fn spawn_avatar_server(settings: Arc<Mutex<Settings>>, stop: Arc<AtomicBool>) {
    thread::spawn(move || {
        let listener = loop {
            if stop.load(Ordering::SeqCst) {
                return;
            }
            let port = settings.lock().listen_port;
            match TcpListener::bind(("0.0.0.0", port)) {
                Ok(listener) => break listener,
                Err(_) => thread::sleep(Duration::from_millis(500)),
            }
        };
        let _ = listener.set_nonblocking(true);
        while !stop.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((stream, _)) => {
                    let key = team_key_bytes(&settings.lock());
                    if let Some(key) = key {
                        // Accepted sockets inherit the listener's nonblocking
                        // flag. A WouldBlock read then drops the photo request.
                        let _ = stream.set_nonblocking(false);
                        avatar::handle_connection(stream, &key);
                    }
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(40));
                }
                Err(_) => thread::sleep(Duration::from_millis(200)),
            }
        }
    });
}

pub(crate) fn start_local_preview(settings: &Settings) -> Result<(), String> {
    if settings.do_not_disturb {
        return Err("Turn off Do Not Disturb to preview on this Mac.".into());
    }
    if settings.tailnet_only {
        return Err("Turn off Accept only Tailscale IPs to preview on this Mac.".into());
    }
    let key =
        team_key_bytes(settings).ok_or_else(|| "Set a team key in Settings first.".to_string())?;
    let port = settings.listen_port;
    let color = settings.sender_color.clone();
    thread::spawn(move || {
        let Ok(socket) = UdpSocket::bind("127.0.0.1:0") else {
            return;
        };
        let dest = SocketAddr::from(([127, 0, 0, 1], port));
        let send = |kind: MessageKind, _packet_color: Option<String>| {
            let mut msg = new_outgoing(kind, "local-preview", "Preview", "", "");
            sign_message(&key, &mut msg);
            if let Ok(bytes) = serde_json::to_vec(&msg) {
                let _ = socket.send_to(&bytes, dest);
            }
        };
        send(MessageKind::Start, Some(color));
        for _ in 0..4 {
            thread::sleep(Duration::from_millis(400));
            send(MessageKind::Heartbeat, None);
        }
        thread::sleep(Duration::from_millis(200));
        send(MessageKind::Stop, None);
    });
    Ok(())
}

pub(crate) fn emit_overlay(
    app: &AppHandle,
    settings: &Arc<Mutex<Settings>>,
    snapshot: OverlaySnapshot,
) {
    let cfg = settings.lock().clone();
    let avatar = match &snapshot {
        OverlaySnapshot::Visible(active) if active.sender_id == "local-preview" => {
            avatar::own_data_url()
        }
        OverlaySnapshot::Visible(active) => avatar::data_url_for_sender(&active.sender_id),
        OverlaySnapshot::Hidden => None,
    };
    let chrome = OverlayChrome::from_settings(&cfg, avatar);
    sync_overlay_windows(app, &snapshot, chrome);
}

/// Lights this machine with the current glow and screen settings. Skips the
/// UDP loopback path, so do-not-disturb and Tailscale-only do not block it.
pub fn show_style_preview(
    app: &AppHandle,
    settings: &Arc<Mutex<Settings>>,
    overlay: &Arc<Mutex<OverlayStateMachine>>,
) {
    let color = color_for_peer(&settings.lock(), "local-preview");
    let snapshot = {
        let mut overlay = overlay.lock();
        overlay.apply_start("local-preview".into(), "Preview".into(), color);
        overlay.snapshot()
    };
    emit_overlay(app, settings, snapshot);
    let app = app.clone();
    let settings = settings.clone();
    let overlay = overlay.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_secs(2));
        let snapshot = {
            let mut overlay = overlay.lock();
            overlay.apply_stop("local-preview");
            overlay.snapshot()
        };
        emit_overlay(&app, &settings, snapshot);
    });
}

struct Discovery {
    force: StdMutex<bool>,
    wake: Condvar,
    refreshed_at: StdMutex<Option<u64>>,
}

impl Discovery {
    fn new() -> Self {
        Self {
            force: StdMutex::new(false),
            wake: Condvar::new(),
            refreshed_at: StdMutex::new(None),
        }
    }
}

const DISCOVERY_INTERVAL: Duration = Duration::from_secs(20);
const DISCOVERY_WATCH: Duration = Duration::from_secs(5);

fn spawn_presence(
    app: AppHandle,
    settings: Arc<Mutex<Settings>>,
    peers: Arc<Mutex<HashMap<String, PeerRecord>>>,
    roster: Arc<Mutex<RosterSync>>,
    discovery: Arc<Discovery>,
    stop: Arc<AtomicBool>,
) {
    thread::spawn(move || {
        let mut seen_targets = BTreeSet::new();
        let mut last_ping = Instant::now() - DISCOVERY_INTERVAL;
        while !stop.load(Ordering::SeqCst) {
            let focused = focus::refresh_system_focus();
            let cfg = {
                let mut cfg = settings.lock();
                if !focused && cfg.suppress_system_dnd {
                    cfg.suppress_system_dnd = false;
                    let _ = crate::settings::save_settings(&cfg);
                }
                let mut map = peers.lock();
                let manual = manual_peer_records(&cfg.manual_peers, cfg.listen_port);
                sync_manual_peers(&mut map, manual);
                if reveal_manual_peers(&mut cfg, &map) {
                    let _ = crate::settings::save_settings(&cfg);
                }
                drop(map);
                cfg.clone()
            };
            let dnd_bit = u8::from(effective_dnd(&cfg, focused));
            if LAST_DND.swap(dnd_bit, Ordering::Relaxed) != dnd_bit {
                let _ = app.emit("dnd-changed", dnd_bit == 1);
            }
            {
                let mut map = peers.lock();
                // A tailnet machine is not a person until it answers with the team key.
                map.retain(|_, peer| peer.source != PeerSource::Tailscale);
            }
            let tailnet = tailscale_targets(cfg.listen_port);
            let target_set: BTreeSet<SocketAddr> = tailnet.iter().copied().collect();
            let joined = target_set != seen_targets;
            let due = last_ping.elapsed() >= DISCOVERY_INTERVAL;
            let forced = {
                let mut flag = discovery.force.lock().expect("discovery lock");
                let was = *flag;
                *flag = false;
                was
            };
            if team_key_bytes(&cfg).is_some() && (joined || due || forced) {
                send_presence(&cfg, &peers, tailnet);
                last_ping = Instant::now();
                seen_targets = target_set;
                let at = unix_millis();
                *discovery.refreshed_at.lock().expect("discovery lock") = Some(at);
                let _ = app.emit("discovery-refreshed", at);
            }
            if team_key_bytes(&cfg).is_some() {
                let list = peer_list_from_map(&peers.lock(), &cfg);
                let _ = app.emit("peers-updated", list);
                let force = {
                    let mut roster = roster.lock();
                    let due = roster.last_forced.elapsed() >= ROSTER_REFRESH;
                    if due {
                        roster.last_forced = Instant::now();
                    }
                    due
                };
                maybe_publish_roster(&cfg, &peers, &roster, force);
            }
            let guard = discovery.force.lock().expect("discovery lock");
            if !*guard && !stop.load(Ordering::SeqCst) {
                let _ = discovery
                    .wake
                    .wait_timeout(guard, DISCOVERY_WATCH)
                    .expect("discovery lock");
            }
        }
    });
}

fn send_presence(
    cfg: &Settings,
    peers: &Arc<Mutex<HashMap<String, PeerRecord>>>,
    tailnet: Vec<SocketAddr>,
) {
    let Some(key) = team_key_bytes(cfg) else {
        return;
    };
    let Ok(socket) = UdpSocket::bind("0.0.0.0:0") else {
        return;
    };
    let mut msg = new_outgoing(
        MessageKind::Presence,
        &stable_sender_id(&cfg.display_name),
        &cfg.display_name,
        &status::current_status(),
        &avatar::current_hash(),
    );
    sign_message(&key, &mut msg);
    let Ok(data) = serde_json::to_vec(&msg) else {
        return;
    };
    if !cfg.tailnet_only {
        broadcast(&socket, &data, cfg.listen_port);
    }
    let targets = {
        let known = peers.lock();
        presence_targets(cfg, &known, tailnet)
    };
    for addr in targets {
        let _ = socket.send_to(&data, addr);
    }
}

fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

fn presence_targets(
    cfg: &Settings,
    peers: &HashMap<String, PeerRecord>,
    mut targets: Vec<SocketAddr>,
) -> Vec<SocketAddr> {
    for peer in peers.values() {
        for addr in &peer.addrs {
            if cfg.tailnet_only && !is_tailnet_ip(&addr.ip()) {
                continue;
            }
            if !targets.contains(addr) {
                targets.push(*addr);
            }
        }
    }
    targets
}

fn broadcast(socket: &UdpSocket, data: &[u8], port: u16) {
    let _ = socket.set_broadcast(true);
    if let Ok(addrs) = if_addrs::get_if_addrs() {
        for iface in addrs {
            if !iface.is_loopback() {
                if let if_addrs::IfAddr::V4(v4) = iface.addr {
                    let ip = v4.ip;
                    if ip.is_private() {
                        let octets = ip.octets();
                        let bcast = SocketAddr::new(
                            IpAddr::V4(std::net::Ipv4Addr::new(
                                octets[0], octets[1], octets[2], 255,
                            )),
                            port,
                        );
                        let _ = socket.send_to(data, bcast);
                    }
                }
            }
        }
    }
}

fn spawn_hold_sender(
    settings: Arc<Mutex<Settings>>,
    hold_target: Arc<Mutex<Option<String>>>,
    hold_addrs: Arc<Mutex<Vec<SocketAddr>>>,
    stop: Arc<AtomicBool>,
) {
    thread::spawn(move || {
        let socket = UdpSocket::bind("0.0.0.0:0").expect("bind hold socket");
        let mut sent_start = false;
        while !stop.load(Ordering::SeqCst) {
            let target = hold_target.lock().clone();
            if let Some(_peer_id) = target {
                let cfg = settings.lock().clone();
                let Some(key) = team_key_bytes(&cfg) else {
                    thread::sleep(Duration::from_millis(100));
                    continue;
                };
                let addrs = hold_addrs.lock().clone();
                let kind = if sent_start {
                    MessageKind::Heartbeat
                } else {
                    MessageKind::Start
                };
                let mut msg = new_outgoing(
                    kind,
                    &stable_sender_id(&cfg.display_name),
                    &cfg.display_name,
                    "",
                    &avatar::current_hash(),
                );
                sign_message(&key, &mut msg);
                if let Ok(data) = serde_json::to_vec(&msg) {
                    for addr in addrs {
                        let _ = socket.send_to(&data, addr);
                    }
                }
                sent_start = true;
                thread::sleep(Duration::from_millis(500));
            } else {
                if sent_start {
                    let cfg = settings.lock().clone();
                    if let Some(key) = team_key_bytes(&cfg) {
                        let mut msg = new_outgoing(
                            MessageKind::Stop,
                            &stable_sender_id(&cfg.display_name),
                            &cfg.display_name,
                            "",
                            "",
                        );
                        sign_message(&key, &mut msg);
                        if let Ok(data) = serde_json::to_vec(&msg) {
                            let addrs = hold_addrs.lock().clone();
                            for addr in addrs {
                                let _ = socket.send_to(&data, addr);
                            }
                        }
                    }
                    hold_addrs.lock().clear();
                }
                sent_start = false;
                thread::sleep(Duration::from_millis(50));
            }
        }
    });
}

fn spawn_mdns(
    app: AppHandle,
    settings: Arc<Mutex<Settings>>,
    peers: Arc<Mutex<HashMap<String, PeerRecord>>>,
    roster: Arc<Mutex<RosterSync>>,
    stop: Arc<AtomicBool>,
) {
    thread::spawn(move || {
        let daemon = match ServiceDaemon::new() {
            Ok(d) => d,
            Err(_) => return,
        };
        while !stop.load(Ordering::SeqCst) {
            let cfg = settings.lock().clone();
            let instance = format!("Tap {}._tap._udp.local.", cfg.display_name);
            let host = format!("{}.local.", cfg.display_name.replace(' ', "-"));
            let port = cfg.listen_port;
            let properties = [("id", stable_sender_id(&cfg.display_name))];
            let info =
                match ServiceInfo::new(SERVICE_TYPE, &instance, &host, "", port, &properties[..]) {
                    Ok(i) => i,
                    Err(_) => {
                        thread::sleep(Duration::from_secs(5));
                        continue;
                    }
                };
            let _ = daemon.register(info);
            let receiver = daemon.browse(SERVICE_TYPE).ok();
            let browse_deadline = Instant::now() + Duration::from_secs(30);
            while Instant::now() < browse_deadline && !stop.load(Ordering::SeqCst) {
                if let Some(recv) = &receiver {
                    while let Ok(event) = recv.try_recv() {
                        if let ServiceEvent::ServiceResolved(info) = event {
                            let full = info.get_fullname();
                            let display_name = full
                                .split('.')
                                .next()
                                .unwrap_or(&full)
                                .trim_start_matches("Tap ")
                                .to_string();
                            let id = info
                                .get_properties()
                                .get("id")
                                .and_then(|v| v.val())
                                .map(|v| String::from_utf8_lossy(v).into_owned())
                                .unwrap_or_else(|| display_name.clone());
                            let addrs: Vec<SocketAddr> = info
                                .get_addresses()
                                .iter()
                                .map(|ip| SocketAddr::new(*ip, port))
                                .collect();
                            if addrs.is_empty() {
                                continue;
                            }
                            let mut map = peers.lock();
                            *map = merge_peer_maps(
                                map.clone(),
                                vec![PeerRecord {
                                    id,
                                    name: display_name,
                                    addrs,
                                    source: PeerSource::Mdns,
                                    last_seen: Instant::now(),
                                    status: String::new(),
                                    avatar_hash: String::new(),
                                    heard_directly: true,
                                }],
                            );
                            drop(map);
                            let _ =
                                app.emit("peers-updated", peer_list_from_map(&peers.lock(), &cfg));
                            maybe_publish_roster(&cfg, &peers, &roster, false);
                        }
                    }
                }
                thread::sleep(Duration::from_millis(200));
            }
            let _ = daemon.unregister(&instance);
            thread::sleep(Duration::from_secs(2));
        }
    });
}

fn spawn_tailscale_poll(
    app: AppHandle,
    settings: Arc<Mutex<Settings>>,
    peers: Arc<Mutex<HashMap<String, PeerRecord>>>,
    roster: Arc<Mutex<RosterSync>>,
    stop: Arc<AtomicBool>,
) {
    thread::spawn(move || {
        while !stop.load(Ordering::SeqCst) {
            let cfg = settings.lock().clone();
            let list = {
                let mut map = peers.lock();
                map.retain(|_, peer| peer.source != PeerSource::Tailscale);
                let manual = manual_peer_records(&cfg.manual_peers, cfg.listen_port);
                sync_manual_peers(&mut map, manual);
                peer_list_from_map(&map, &cfg)
            };
            let _ = app.emit("peers-updated", list);
            maybe_publish_roster(&cfg, &peers, &roster, false);
            thread::sleep(Duration::from_secs(15));
        }
    });
}

pub fn stable_sender_id(display_name: &str) -> String {
    use sha2::{Digest, Sha256};
    let host = hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(host.as_bytes());
    hasher.update(b"|");
    hasher.update(display_name.as_bytes());
    format!("{:x}", hasher.finalize())[..16].to_string()
}

pub fn resolve_hold_addrs(peers: &HashMap<String, PeerRecord>, peer_id: &str) -> Vec<SocketAddr> {
    addresses_for_peer(peers, peer_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn preview_settings(port: u16) -> Settings {
        Settings {
            team_key: "suite-key".into(),
            listen_port: port,
            sender_color: "#3b82f6".into(),
            do_not_disturb: false,
            tailnet_only: false,
            ..Settings::default()
        }
    }

    fn recv_message(socket: &UdpSocket) -> TapMessage {
        let mut buf = [0u8; 4096];
        let (len, _) = socket
            .recv_from(&mut buf)
            .expect("timed out waiting for a preview packet");
        serde_json::from_slice(&buf[..len]).expect("preview packet was not JSON")
    }

    #[test]
    fn local_preview_over_udp_shows_then_clears_overlay() {
        let socket = UdpSocket::bind("127.0.0.1:0").expect("bind preview receiver");
        socket
            .set_read_timeout(Some(Duration::from_secs(4)))
            .expect("read timeout");
        let port = socket.local_addr().expect("local addr").port();
        let settings = preview_settings(port);
        start_local_preview(&settings).expect("preview starts");

        let mut overlay = OverlayStateMachine::default();
        let mut peers = HashMap::new();
        let mut replay = ReplayGuard::default();
        let mut roster = RosterSync::default();
        let mut saw_start = false;
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            if Instant::now() > deadline {
                panic!("preview did not send stop");
            }
            let msg = recv_message(&socket);
            let effect = apply_inbound(
                &settings,
                &mut overlay,
                &mut peers,
                &mut replay,
                &mut roster,
                msg.clone(),
                IpAddr::V4(Ipv4Addr::LOCALHOST),
                None,
            );
            match msg.kind {
                MessageKind::Start => {
                    saw_start = true;
                    assert!(matches!(
                        effect,
                        InboundEffect::OverlayChanged(OverlaySnapshot::Visible(_))
                    ));
                    assert!(matches!(overlay.snapshot(), OverlaySnapshot::Visible(_)));
                }
                MessageKind::Heartbeat => {
                    assert!(saw_start);
                    assert!(matches!(overlay.snapshot(), OverlaySnapshot::Visible(_)));
                }
                MessageKind::Stop => {
                    assert!(saw_start);
                    assert!(matches!(overlay.snapshot(), OverlaySnapshot::Hidden));
                    break;
                }
                MessageKind::Presence | MessageKind::Busy | MessageKind::Roster => {}
            }
        }
    }

    #[test]
    fn preview_refuses_dnd_and_tailnet_only() {
        let mut settings = preview_settings(1);
        settings.do_not_disturb = true;
        assert!(start_local_preview(&settings).is_err());

        settings.do_not_disturb = false;
        settings.tailnet_only = true;
        assert!(start_local_preview(&settings).is_err());
    }

    #[test]
    fn bad_signature_and_missing_key_do_not_show_overlay() {
        let settings = preview_settings(9);
        let mut overlay = OverlayStateMachine::default();
        let mut peers = HashMap::new();
        let mut replay = ReplayGuard::default();
        let mut roster = RosterSync::default();
        let mut msg = new_outgoing(MessageKind::Start, "alice", "Alice", "", "");
        sign_message(b"other-key", &mut msg);
        let effect = apply_inbound(
            &settings,
            &mut overlay,
            &mut peers,
            &mut replay,
            &mut roster,
            msg,
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            None,
        );
        assert!(matches!(effect, InboundEffect::Ignored));
        assert!(matches!(overlay.snapshot(), OverlaySnapshot::Hidden));

        let mut unsigned = settings.clone();
        unsigned.team_key.clear();
        let mut msg = new_outgoing(MessageKind::Start, "alice", "Alice", "", "");
        sign_message(b"suite-key", &mut msg);
        let effect = apply_inbound(
            &unsigned,
            &mut overlay,
            &mut peers,
            &mut replay,
            &mut roster,
            msg,
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            None,
        );
        assert!(matches!(effect, InboundEffect::Ignored));
        assert!(matches!(overlay.snapshot(), OverlaySnapshot::Hidden));
    }

    fn signed_roster(
        settings: &Settings,
        sender_id: &str,
        sender_name: &str,
        members: Vec<RosterMember>,
    ) -> TapMessage {
        let mut msg = new_outgoing(MessageKind::Roster, sender_id, sender_name, "", "");
        msg.members = members;
        sign_message(&team_key_bytes(settings).expect("key"), &mut msg);
        msg
    }

    fn apply_roster_msg(
        settings: &Settings,
        peers: &mut HashMap<String, PeerRecord>,
        roster: &mut RosterSync,
        msg: TapMessage,
    ) -> InboundEffect {
        let mut overlay = OverlayStateMachine::default();
        let mut replay = ReplayGuard::default();
        apply_inbound(
            settings,
            &mut overlay,
            peers,
            &mut replay,
            roster,
            msg,
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)),
            None,
        )
    }

    #[test]
    fn a_join_reaches_the_rest_of_the_team_and_a_leave_sticks() {
        let settings = preview_settings(37561);
        let mut peers = HashMap::new();
        let mut roster = RosterSync::default();
        let effect = apply_roster_msg(
            &settings,
            &mut peers,
            &mut roster,
            signed_roster(
                &settings,
                "ryan",
                "Ryan",
                vec![
                    RosterMember {
                        id: "ryan".into(),
                        name: "Ryan".into(),
                        addrs: vec!["10.0.0.2:37561".into()],
                        gone: false,
                    },
                    RosterMember {
                        id: "cole".into(),
                        name: "Cole".into(),
                        addrs: vec!["10.0.0.3:37561".into()],
                        gone: false,
                    },
                ],
            ),
        );
        assert!(matches!(effect, InboundEffect::PeersChanged));
        assert!(peers.contains_key("cole"));
        assert!(peers.contains_key("ryan"));

        let announced = roster_members(&settings, &peers, &roster.gone);
        assert!(announced
            .iter()
            .any(|member| member.id == "cole" && !member.gone));

        let effect = apply_roster_msg(
            &settings,
            &mut peers,
            &mut roster,
            signed_roster(
                &settings,
                "cole",
                "Cole",
                vec![RosterMember {
                    id: "cole".into(),
                    name: "Cole".into(),
                    addrs: vec![],
                    gone: true,
                }],
            ),
        );
        assert!(matches!(effect, InboundEffect::PeersChanged));
        assert!(!peers.contains_key("cole"));
        assert!(roster.gone.contains_key("cole"));

        let effect = apply_roster_msg(
            &settings,
            &mut peers,
            &mut roster,
            signed_roster(
                &settings,
                "ryan",
                "Ryan",
                vec![RosterMember {
                    id: "cole".into(),
                    name: "Cole".into(),
                    addrs: vec!["10.0.0.3:37561".into()],
                    gone: false,
                }],
            ),
        );
        assert!(matches!(effect, InboundEffect::Ignored));
        assert!(!peers.contains_key("cole"));

        let mut back = new_outgoing(MessageKind::Presence, "cole", "Cole", "", "");
        sign_message(&team_key_bytes(&settings).expect("key"), &mut back);
        let effect = apply_roster_msg(&settings, &mut peers, &mut roster, back);
        assert!(matches!(effect, InboundEffect::PeersChanged));
        assert!(peers.contains_key("cole"));
        assert!(!roster.gone.contains_key("cole"));
    }
}
