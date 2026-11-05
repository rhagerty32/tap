use crate::discovery::{discover_tailscale_peers, manual_peer_records, merge_peer_maps, peer_list_from_map};
use crate::overlay_state::{OverlaySnapshot, OverlayStateMachine};
use crate::peer::{PeerRecord, PeerSource};
use crate::protocol::{
    new_outgoing, sign_message, verify_message, MessageKind, ReplayGuard, TapMessage,
};
use crate::settings::{is_tailnet_ip, team_key_bytes, Settings};
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use crate::overlay::sync_overlay_windows;
use tauri::{AppHandle, Emitter};

const SERVICE_TYPE: &str = "_tap._udp.local.";

pub struct NetworkRuntime {
    stop: Arc<AtomicBool>,
    hold_target: Arc<Mutex<Option<String>>>,
    hold_addrs: Arc<Mutex<Vec<SocketAddr>>>,
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

        spawn_receiver(
            app.clone(),
            settings.clone(),
            overlay.clone(),
            peers.clone(),
            replay.clone(),
            stop.clone(),
        );
        spawn_presence(app.clone(), settings.clone(), peers.clone(), stop.clone());
        spawn_hold_sender(
            settings.clone(),
            hold_target.clone(),
            hold_addrs.clone(),
            stop.clone(),
        );
        spawn_mdns(app.clone(), settings.clone(), peers.clone(), stop.clone());
        spawn_tailscale_poll(app.clone(), settings.clone(), peers.clone(), stop.clone());

        Self {
            stop,
            hold_target,
            hold_addrs,
        }
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    pub fn begin_hold(&self, peer_id: &str, addrs: Vec<SocketAddr>) {
        *self.hold_target.lock() = Some(peer_id.to_string());
        *self.hold_addrs.lock() = addrs;
    }

    pub fn end_hold(&self) {
        *self.hold_target.lock() = None;
        // hold thread sends STOP using last known addrs, then clears them
        thread::spawn({
            let hold_addrs = self.hold_addrs.clone();
            move || {
                thread::sleep(Duration::from_millis(100));
                hold_addrs.lock().clear();
            }
        });
    }
}

fn spawn_receiver(
    app: AppHandle,
    settings: Arc<Mutex<Settings>>,
    overlay: Arc<Mutex<OverlayStateMachine>>,
    peers: Arc<Mutex<HashMap<String, PeerRecord>>>,
    replay: Arc<Mutex<ReplayGuard>>,
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

        let mut buf = [0u8; 4096];
        let mut last_tick = Instant::now();
        while !stop.load(Ordering::SeqCst) {
            loop {
                match socket.recv_from(&mut buf) {
                    Ok((len, src)) => {
                        if let Ok(msg) = serde_json::from_slice::<TapMessage>(&buf[..len]) {
                            handle_inbound(
                                &app,
                                &settings,
                                &overlay,
                                &peers,
                                &replay,
                                msg,
                                src.ip(),
                            );
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(_) => break,
                }
            }

            if last_tick.elapsed() >= Duration::from_millis(200) {
                last_tick = Instant::now();
                let changed = {
                    let mut o = overlay.lock();
                    o.tick(Instant::now());
                    o.snapshot()
                };
                emit_overlay(&app, &settings, changed);
            }
            thread::sleep(Duration::from_millis(20));
        }
    });
}

fn handle_inbound(
    app: &AppHandle,
    settings: &Arc<Mutex<Settings>>,
    overlay: &Arc<Mutex<OverlayStateMachine>>,
    peers: &Arc<Mutex<HashMap<String, PeerRecord>>>,
    replay: &Arc<Mutex<ReplayGuard>>,
    msg: TapMessage,
    src_ip: IpAddr,
) {
    let cfg = settings.lock().clone();
    if cfg.tailnet_only && !is_tailnet_ip(&src_ip) {
        return;
    }
    let Some(key) = team_key_bytes(&cfg) else {
        return;
    };
    {
        let mut guard = replay.lock();
        if verify_message(&key, &msg, &mut guard).is_err() {
            return;
        }
    }

    if cfg.do_not_disturb && matches!(msg.kind, MessageKind::Start | MessageKind::Heartbeat) {
        return;
    }
    if cfg.mute_senders.get(&msg.sender_id).copied().unwrap_or(false) {
        return;
    }
    if cfg.allow_senders.values().any(|v| *v)
        && !cfg.allow_senders.get(&msg.sender_id).copied().unwrap_or(false)
    {
        return;
    }

    match msg.kind {
        MessageKind::Presence => {
            let mut map = peers.lock();
            *map = merge_peer_maps(
                map.clone(),
                vec![PeerRecord {
                    id: msg.sender_id.clone(),
                    name: msg.sender_name.clone(),
                    addrs: vec![SocketAddr::new(src_ip, cfg.listen_port)],
                    source: PeerSource::Mdns,
                    last_seen: Instant::now(),
                }],
            );
            let _ = app.emit("peers-updated", peer_list_from_map(&map, &cfg));
        }
        MessageKind::Start => {
            let color = msg.color.unwrap_or_else(|| cfg.sender_color.clone());
            {
                let mut o = overlay.lock();
                o.apply_start(msg.sender_id.clone(), msg.sender_name.clone(), color);
            }
            emit_overlay(app, settings, overlay.lock().snapshot());
        }
        MessageKind::Heartbeat => {
            overlay.lock().apply_heartbeat(&msg.sender_id);
        }
        MessageKind::Stop => {
            overlay.lock().apply_stop(&msg.sender_id);
            emit_overlay(app, settings, overlay.lock().snapshot());
        }
    }
}

fn emit_overlay(app: &AppHandle, settings: &Arc<Mutex<Settings>>, snapshot: OverlaySnapshot) {
    sync_overlay_windows(app, &snapshot);
    let mut payload = overlay_payload(snapshot);
    if let Some(obj) = payload.as_object_mut() {
        obj.insert(
            "chimeEnabled".into(),
            serde_json::json!(settings.lock().chime_enabled),
        );
    }
    let _ = app.emit("overlay-changed", payload);
}

pub fn overlay_payload(snapshot: OverlaySnapshot) -> serde_json::Value {
    match snapshot {
        OverlaySnapshot::Hidden => serde_json::json!({ "visible": false }),
        OverlaySnapshot::Visible(v) => serde_json::json!({
            "visible": true,
            "senderName": v.sender_name,
            "color": v.color,
        }),
    }
}

fn spawn_presence(
    app: AppHandle,
    settings: Arc<Mutex<Settings>>,
    peers: Arc<Mutex<HashMap<String, PeerRecord>>>,
    stop: Arc<AtomicBool>,
) {
    thread::spawn(move || {
        while !stop.load(Ordering::SeqCst) {
            let cfg = settings.lock().clone();
            if let Some(key) = team_key_bytes(&cfg) {
                if let Ok(socket) = UdpSocket::bind("0.0.0.0:0") {
                    let mut msg = new_outgoing(
                        MessageKind::Presence,
                        &stable_sender_id(&cfg.display_name),
                        &cfg.display_name,
                        None,
                    );
                    sign_message(&key, &mut msg);
                    if let Ok(data) = serde_json::to_vec(&msg) {
                        broadcast(&socket, &data, cfg.listen_port);
                    }
                }
                let list = peer_list_from_map(&peers.lock(), &cfg);
                let _ = app.emit("peers-updated", list);
            }
            thread::sleep(Duration::from_secs(5));
        }
    });
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
                    Some(cfg.sender_color.clone()),
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
                            None,
                        );
                        sign_message(&key, &mut msg);
                        if let Ok(data) = serde_json::to_vec(&msg) {
                            let addrs = hold_addrs.lock().clone();
                            for addr in addrs {
                                let _ = socket.send_to(&data, addr);
                            }
                        }
                    }
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
            let info = match ServiceInfo::new(
                SERVICE_TYPE,
                &instance,
                &host,
                "",
                port,
                &properties[..],
            ) {
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
                                }],
                            );
                            let _ = app.emit("peers-updated", peer_list_from_map(&map, &cfg));
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
    stop: Arc<AtomicBool>,
) {
    thread::spawn(move || {
        while !stop.load(Ordering::SeqCst) {
            let cfg = settings.lock().clone();
            let mut map = peers.lock();
            let ts = discover_tailscale_peers(cfg.listen_port);
            let manual = manual_peer_records(&cfg.manual_peers, cfg.listen_port);
            *map = merge_peer_maps(map.clone(), ts);
            *map = merge_peer_maps(map.clone(), manual);
            let _ = app.emit("peers-updated", peer_list_from_map(&map, &cfg));
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

pub fn resolve_hold_addrs(
    peers: &HashMap<String, PeerRecord>,
    peer_id: &str,
) -> Vec<SocketAddr> {
    peers
        .get(peer_id)
        .map(|p| p.addrs.clone())
        .unwrap_or_default()
}
