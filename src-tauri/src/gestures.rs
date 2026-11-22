//! Mac trackpad host. The Swift library in `native/gestures` reads the pad,
//! changes volume, brightness, and media, and draws the glass level panel.
//! This module loads that library and turns a corner rest into a hold.

use crate::network::resolve_hold_addrs;
use crate::settings::{Settings, TrackpadSettings};
use crate::{perform_alignment_haptic, set_tray_sending, AppState};
use libloading::{Library, Symbol};
use parking_lot::Mutex;
use serde::Serialize;
use std::ffi::CString;
use std::os::raw::c_char;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

type SlideFn = unsafe extern "C" fn(i32, i32, f64);
type CommandFn = unsafe extern "C" fn(i32, f64, i32);
type HoldFn = unsafe extern "C" fn(i32, i32);
type StartFn = unsafe extern "C" fn(SlideFn, CommandFn, HoldFn) -> i32;
type ApplyFn = unsafe extern "C" fn(*const c_char) -> i32;
type StopFn = unsafe extern "C" fn();

struct Host {
    _library: Library,
    apply: ApplyFn,
    stop: StopFn,
}

static HOST: Mutex<Option<Host>> = Mutex::new(None);
static APP: OnceLock<AppHandle> = OnceLock::new();
static HUD_GEN: AtomicU64 = AtomicU64::new(0);
static CORNER_HOLDING: AtomicBool = AtomicBool::new(false);
static HUD_PAYLOAD: Mutex<Option<serde_json::Value>> = Mutex::new(None);

#[derive(Serialize)]
struct HudPayload {
    visible: bool,
    kind: String,
    level: f64,
    muted: bool,
}

pub fn install(app: &AppHandle) {
    let _ = APP.set(app.clone());
    let Some(path) = library_path() else {
        eprintln!("tap: trackpad library not found");
        return;
    };
    let library = match unsafe { Library::new(&path) } {
        Ok(library) => library,
        Err(err) => {
            eprintln!("tap: could not load {}: {err}", path.display());
            return;
        }
    };
    let start: Symbol<StartFn> = match unsafe { library.get(b"tap_gestures_start\0") } {
        Ok(start) => start,
        Err(err) => {
            eprintln!("tap: trackpad library is missing tap_gestures_start: {err}");
            return;
        }
    };
    let apply: Symbol<ApplyFn> = match unsafe { library.get(b"tap_gestures_apply_config\0") } {
        Ok(apply) => apply,
        Err(err) => {
            eprintln!("tap: trackpad library is missing tap_gestures_apply_config: {err}");
            return;
        }
    };
    let stop: Symbol<StopFn> = match unsafe { library.get(b"tap_gestures_stop\0") } {
        Ok(stop) => stop,
        Err(err) => {
            eprintln!("tap: trackpad library is missing tap_gestures_stop: {err}");
            return;
        }
    };
    let code = unsafe { start(on_slide, on_command, on_hold) };
    if code == 2 {
        eprintln!("tap: this Mac has no built-in trackpad");
    }
    let apply_fn = *apply;
    let stop_fn = *stop;
    *HOST.lock() = Some(Host {
        _library: library,
        apply: apply_fn,
        stop: stop_fn,
    });
}

pub fn apply(settings: &Settings) {
    let host = HOST.lock();
    let Some(host) = host.as_ref() else {
        return;
    };
    let Ok(json) = serde_json::to_string(&settings.trackpad) else {
        return;
    };
    let Ok(cjson) = CString::new(json) else {
        return;
    };
    let code = unsafe { (host.apply)(cjson.as_ptr()) };
    if code != 0 {
        eprintln!("tap: trackpad settings were rejected");
    }
}

pub fn shutdown() {
    let host = HOST.lock();
    if let Some(host) = host.as_ref() {
        unsafe { (host.stop)() };
    }
}

fn library_path() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("libTapGestures.dylib"));
            candidates.push(dir.join("../Frameworks/libTapGestures.dylib"));
            candidates.push(dir.join("../Resources/libTapGestures.dylib"));
        }
    }
    candidates.push(PathBuf::from(env!("TAP_GESTURES_LIB")));
    candidates.into_iter().find(|path| path.is_file())
}

extern "C" fn on_slide(target: i32, phase: i32, level: f64) {
    let kind = if target == 0 { "volume" } else { "brightness" };
    present(kind, level, false, phase == 2);
}

extern "C" fn on_command(command: i32, level: f64, muted: i32) {
    let kind = match command {
        0 => "play",
        1 => "next",
        2 => "previous",
        _ => "mute",
    };
    present(kind, level, muted != 0, true);
}

extern "C" fn on_hold(corner: i32, phase: i32) {
    let Some(app) = APP.get().cloned() else {
        return;
    };
    let _ = app.clone().run_on_main_thread(move || {
        if phase == 1 {
            begin_corner_hold(&app, corner);
        } else {
            end_corner_hold(&app);
        }
    });
}

fn begin_corner_hold(app: &AppHandle, corner: i32) {
    let state = app.state::<AppState>();
    let settings = state.settings.lock().clone();
    let peer_id = peer_for_corner(&settings.trackpad, corner);
    if peer_id.is_empty() {
        return;
    }
    let addrs = resolve_hold_addrs(&state.peers.lock(), &peer_id);
    if addrs.is_empty() {
        eprintln!("tap: {peer_id} has no reachable address");
        return;
    }
    if let Some(net) = state.network.lock().as_ref() {
        net.begin_hold(&peer_id, addrs);
    }
    CORNER_HOLDING.store(true, Ordering::Relaxed);
    set_tray_sending(app, true);
    perform_alignment_haptic();
}

fn end_corner_hold(app: &AppHandle) {
    if !CORNER_HOLDING.swap(false, Ordering::Relaxed) {
        return;
    }
    let state = app.state::<AppState>();
    if let Some(net) = state.network.lock().as_ref() {
        net.end_hold();
    }
    set_tray_sending(app, false);
}

fn peer_for_corner(settings: &TrackpadSettings, corner: i32) -> String {
    let actions = match corner {
        0 => &settings.corners.top_left,
        1 => &settings.corners.top_right,
        2 => &settings.corners.bottom_left,
        3 => &settings.corners.bottom_right,
        _ => return String::new(),
    };
    actions.hold_peer.clone()
}

fn present(kind: &str, level: f64, muted: bool, dismiss: bool) {
    let Some(app) = APP.get().cloned() else {
        return;
    };
    let payload = HudPayload {
        visible: true,
        kind: kind.to_string(),
        level,
        muted,
    };
    let gen = HUD_GEN.fetch_add(1, Ordering::Relaxed) + 1;
    let shown = app.clone();
    let _ = app.clone().run_on_main_thread(move || show_hud(&shown, &payload));
    if dismiss {
        let hiding = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(700));
            if HUD_GEN.load(Ordering::Relaxed) != gen {
                return;
            }
            let window = hiding.clone();
            let _ = hiding.run_on_main_thread(move || hide_hud(&window));
        });
    }
}

fn show_hud(app: &AppHandle, payload: &HudPayload) {
    let value = serde_json::to_value(payload).unwrap_or(serde_json::json!({ "visible": false }));
    *HUD_PAYLOAD.lock() = Some(value.clone());
    let Some(window) = hud_window(app) else {
        return;
    };
    place_hud(app, &window);
    let _ = window.emit("trackpad-hud", value);
    let _ = window.show();
}

fn hide_hud(app: &AppHandle) {
    *HUD_PAYLOAD.lock() = None;
    if let Some(window) = app.get_webview_window("hud") {
        let _ = window.emit(
            "trackpad-hud",
            serde_json::json!({ "visible": false, "kind": "", "level": 0, "muted": false }),
        );
        let _ = window.hide();
    }
}

fn hud_window(app: &AppHandle) -> Option<tauri::WebviewWindow> {
    if let Some(window) = app.get_webview_window("hud") {
        return Some(window);
    }
    let window = WebviewWindowBuilder::new(app, "hud", WebviewUrl::App("hud.html".into()))
        .transparent(true)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible(false)
        .focused(false)
        .resizable(false)
        .inner_size(240.0, 64.0)
        .visible_on_all_workspaces(true)
        .on_page_load(|window, load| {
            if load.event() != tauri::webview::PageLoadEvent::Finished {
                return;
            }
            let Some(payload) = HUD_PAYLOAD.lock().clone() else {
                return;
            };
            let _ = window.emit("trackpad-hud", payload);
        })
        .build()
        .ok()?;
    let _ = window.set_ignore_cursor_events(true);
    Some(window)
}

fn place_hud(app: &AppHandle, window: &tauri::WebviewWindow) {
    let Ok(Some(monitor)) = app.primary_monitor() else {
        return;
    };
    let scale = monitor.scale_factor();
    let size = monitor.size();
    let origin = monitor.position();
    let width = 240.0;
    let x = origin.x as f64 / scale + (size.width as f64 / scale - width) / 2.0;
    let y = origin.y as f64 / scale + 36.0;
    let _ = window.set_position(tauri::Position::Logical(tauri::LogicalPosition { x, y }));
}
