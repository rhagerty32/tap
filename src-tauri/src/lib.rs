mod avatar;
mod discovery;
mod focus;
mod network;
mod overlay;
mod overlay_state;
mod peer;
mod protocol;
mod settings;
mod status;

use crate::peer::{PeerInfo, PeerRecord};
use discovery::{hide_person, peer_list_from_map};
use network::{resolve_hold_addrs, stable_sender_id, NetworkRuntime};
use overlay_state::OverlayStateMachine;
use parking_lot::Mutex;
use settings::{
    canonical_color, effective_dnd, generate_team_key, load_settings, normalize_glow,
    normalize_screens, normalize_theme, save_settings, settings_path, Settings,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, RunEvent, State, WebviewWindow, WindowEvent,
};

struct AppState {
    settings: Arc<Mutex<Settings>>,
    peers: Arc<Mutex<HashMap<String, PeerRecord>>>,
    overlay: Arc<Mutex<OverlayStateMachine>>,
    network: Mutex<Option<NetworkRuntime>>,
}

#[tauri::command]
fn get_settings(state: State<AppState>) -> Settings {
    state.settings.lock().clone()
}

#[tauri::command]
fn save_app_settings(
    state: State<AppState>,
    settings: Settings,
    app: AppHandle,
) -> Result<(), String> {
    let saved = {
        let mut current = state.settings.lock();
        let mut incoming = settings;
        incoming.do_not_disturb = current.do_not_disturb;
        incoming.suppress_system_dnd = if incoming.follow_system_dnd {
            current.suppress_system_dnd
        } else {
            false
        };
        incoming.peer_colors = current.peer_colors.clone();
        incoming.peer_keys = current.peer_keys.clone();
        incoming.roster_shortcut = current.roster_shortcut.clone();
        incoming.glow_style = normalize_glow(&incoming.glow_style);
        incoming.overlay_screens = normalize_screens(&incoming.overlay_screens);
        incoming.theme = normalize_theme(&incoming.theme);
        save_settings(&incoming)?;
        *current = incoming.clone();
        incoming
    };
    configure_autostart(&app, saved.launch_at_login)?;
    apply_app_theme(&app, &saved.theme);
    let _ = app.emit("settings-changed", saved.clone());
    let _ = app.emit(
        "dnd-changed",
        effective_dnd(&saved, focus::cached_system_focus()),
    );
    Ok(())
}

#[tauri::command]
fn dnd_state(state: State<AppState>) -> bool {
    effective_dnd(&state.settings.lock(), focus::cached_system_focus())
}

#[tauri::command]
fn set_do_not_disturb(state: State<AppState>, enabled: bool, app: AppHandle) -> Result<(), String> {
    let focused = focus::cached_system_focus();
    let saved = {
        let mut cfg = state.settings.lock();
        if enabled {
            cfg.do_not_disturb = true;
            cfg.suppress_system_dnd = false;
        } else {
            cfg.do_not_disturb = false;
            cfg.suppress_system_dnd = cfg.follow_system_dnd && focused;
        }
        save_settings(&cfg)?;
        cfg.clone()
    };
    let _ = app.emit("settings-changed", saved.clone());
    let _ = app.emit("dnd-changed", effective_dnd(&saved, focused));
    Ok(())
}

#[tauri::command]
fn remove_person(
    state: State<AppState>,
    peer_id: String,
    app: AppHandle,
) -> Result<Settings, String> {
    let map = state.peers.lock().clone();
    let saved = {
        let mut cfg = state.settings.lock();
        hide_person(&mut cfg, &map, &peer_id);
        save_settings(&cfg)?;
        cfg.clone()
    };
    let _ = app.emit(
        "peers-updated",
        peer_list_from_map(&state.peers.lock(), &saved),
    );
    let _ = app.emit("settings-changed", saved.clone());
    Ok(saved)
}

#[tauri::command]
fn refresh_peers(state: State<AppState>) -> Result<(), String> {
    let network = state.network.lock();
    let Some(network) = network.as_ref() else {
        return Err("Tap is still starting.".into());
    };
    network.request_refresh();
    Ok(())
}

#[tauri::command]
fn discovery_refreshed_at(state: State<AppState>) -> Option<u64> {
    state
        .network
        .lock()
        .as_ref()
        .and_then(|network| network.refreshed_at())
}

#[tauri::command]
fn set_peer_key(
    state: State<AppState>,
    peer_id: String,
    key: String,
    app: AppHandle,
) -> Result<Settings, String> {
    let peer_id = peer_id.trim().to_string();
    if peer_id.is_empty() {
        return Err("Choose a person first.".into());
    }
    let names: HashMap<String, String> = state
        .peers
        .lock()
        .iter()
        .map(|(id, peer)| (id.clone(), peer.name.clone()))
        .collect();
    let group = discovery::ids_for_person(&state.peers.lock(), &peer_id);
    let group_refs: Vec<&str> = group.iter().map(String::as_str).collect();
    let cfg = {
        let mut cfg = state.settings.lock();
        if let Err(err) = settings::assign_hotkey(&mut cfg, &peer_id, &key, &group_refs) {
            if let Some(owner_id) = err.strip_prefix("taken:") {
                let name = names.get(owner_id).cloned().unwrap_or_else(|| "someone".into());
                let shown = settings::normalize_hotkey(&key).unwrap_or_default();
                let shown = if shown.is_empty() {
                    key.trim().to_string()
                } else {
                    shown.to_ascii_uppercase()
                };
                return Err(format!("{shown} is already {name}'s key."));
            }
            return Err(err);
        }
        save_settings(&cfg)?;
        cfg.clone()
    };
    let list = peer_list_from_map(&state.peers.lock(), &cfg);
    let _ = app.emit("peers-updated", list);
    let _ = app.emit("settings-changed", cfg.clone());
    Ok(cfg)
}

#[tauri::command]
fn set_peer_color(
    state: State<AppState>,
    peer_id: String,
    color: String,
    app: AppHandle,
) -> Result<(), String> {
    let Some(color) = canonical_color(&color) else {
        return Err("Unknown color.".into());
    };
    let cfg = {
        let mut cfg = state.settings.lock();
        cfg.peer_colors.insert(peer_id, color);
        save_settings(&cfg)?;
        cfg.clone()
    };
    let list = peer_list_from_map(&state.peers.lock(), &cfg);
    let _ = app.emit("peers-updated", list);
    let _ = app.emit("settings-changed", cfg);
    Ok(())
}

#[tauri::command]
fn set_avatar(bytes: Vec<u8>) -> Result<String, String> {
    avatar::store_avatar(&bytes)
}

#[tauri::command]
fn my_avatar() -> Option<String> {
    avatar::own_data_url()
}

#[tauri::command]
fn peer_avatar(peer_id: String, hash: String) -> Option<String> {
    avatar::data_url_for(&peer_id, &hash)
}

#[tauri::command]
fn list_displays(app: AppHandle) -> Vec<String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let app_main = app.clone();
    if app
        .run_on_main_thread(move || {
            let _ = tx.send(overlay::display_names(&app_main));
        })
        .is_err()
    {
        return Vec::new();
    }
    rx.recv_timeout(std::time::Duration::from_secs(1))
        .unwrap_or_default()
}

#[tauri::command]
fn preview_glow(state: State<AppState>, app: AppHandle) {
    network::show_style_preview(&app, &state.settings, &state.overlay);
}

#[tauri::command]
fn list_peers(state: State<AppState>) -> Vec<PeerInfo> {
    let cfg = state.settings.lock().clone();
    peer_list_from_map(&state.peers.lock(), &cfg)
}

#[tauri::command]
fn hold_start(state: State<AppState>, peer_id: String, app: AppHandle) -> Result<(), String> {
    let cfg = state.settings.lock().clone();
    if team_key_missing(&cfg) {
        return Err("Set a team key in Settings before tapping.".into());
    }
    let addrs = resolve_hold_addrs(&state.peers.lock(), &peer_id);
    if addrs.is_empty() {
        return Err("Peer has no reachable address.".into());
    }
    if let Some(net) = state.network.lock().as_ref() {
        net.begin_hold(&peer_id, addrs);
    }
    set_tray_sending(&app, true);
    #[cfg(target_os = "macos")]
    perform_alignment_haptic();
    Ok(())
}

#[tauri::command]
fn hold_stop(state: State<AppState>, app: AppHandle) {
    if let Some(net) = state.network.lock().as_ref() {
        net.end_hold();
    }
    set_tray_sending(&app, false);
}

#[tauri::command]
fn preview_local(state: State<AppState>) -> Result<(), String> {
    let cfg = state.settings.lock().clone();
    network::start_local_preview(&cfg)
}

#[tauri::command]
fn sender_id(state: State<AppState>) -> String {
    let cfg = state.settings.lock().clone();
    stable_sender_id(&cfg.display_name)
}

fn team_key_missing(cfg: &Settings) -> bool {
    cfg.team_key.trim().is_empty()
}

fn configure_autostart(app: &AppHandle, enabled: bool) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;
    if enabled {
        app.autolaunch().enable().map_err(|e| e.to_string())
    } else {
        app.autolaunch().disable().map_err(|e| e.to_string())
    }
}

static TRAY_SENDING: AtomicBool = AtomicBool::new(false);
static KEYBOARD_ROSTER: AtomicBool = AtomicBool::new(false);
static TRAY_RECEIVING: AtomicBool = AtomicBool::new(false);

pub(crate) fn set_tray_sending(app: &AppHandle, sending: bool) {
    TRAY_SENDING.store(sending, Ordering::Relaxed);
    refresh_tray_icon(app);
}

pub(crate) fn set_tray_receiving(app: &AppHandle, receiving: bool) {
    TRAY_RECEIVING.store(receiving, Ordering::Relaxed);
    refresh_tray_icon(app);
}

fn apply_app_theme(app: &AppHandle, theme: &str) {
    let native = match normalize_theme(theme).as_str() {
        "light" => Some(tauri::Theme::Light),
        "dark" => Some(tauri::Theme::Dark),
        _ => None,
    };
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.set_theme(native);
    }
    refresh_tray_icon(app);
}

fn refresh_tray_icon(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    {
        let Some(tray) = app.tray_by_id("main") else {
            return;
        };
        let busy = TRAY_SENDING.load(Ordering::Relaxed) || TRAY_RECEIVING.load(Ordering::Relaxed);
        if busy {
            let dark = app
                .get_webview_window("main")
                .and_then(|window| window.theme().ok())
                == Some(tauri::Theme::Dark);
            let bytes: &[u8] = if dark {
                include_bytes!("../icons/tray-icon-busy-dark.png")
            } else {
                include_bytes!("../icons/tray-icon-busy-light.png")
            };
            if let Ok(icon) = tauri::image::Image::from_bytes(bytes) {
                let _ = tray.set_icon_with_as_template(Some(icon), false);
            }
        } else if let Ok(icon) =
            tauri::image::Image::from_bytes(include_bytes!("../icons/tray-icon@2x.png"))
        {
            let _ = tray.set_icon_with_as_template(Some(icon), true);
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
    }
}

#[cfg(target_os = "macos")]
fn perform_alignment_haptic() {
    unsafe {
        use objc2::runtime::{AnyClass, AnyObject};
        // objc2 0.5 looks classes up by name string. A c"..." literal is a CStr.
        let Some(cls) = AnyClass::get("NSHapticFeedbackManager") else {
            return;
        };
        let performer: *mut AnyObject = objc2::msg_send![cls, defaultPerformer];
        if performer.is_null() {
            return;
        }
        // NSHapticFeedbackPatternAlignment = 1, NSHapticFeedbackPerformanceTimeNow = 1.
        let _: () =
            objc2::msg_send![performer, performFeedbackPattern: 1isize, performanceTime: 1usize];
    }
}

fn keep_off_taskbar(window: &WebviewWindow) {
    let _ = window.set_skip_taskbar(true);
}

fn present_popover(window: &WebviewWindow, immediate_focus: bool) {
    #[cfg(target_os = "macos")]
    if order_popover_front(window, immediate_focus) {
        return;
    }
    let _ = window.show();
    let _ = window.set_focus();
}

#[cfg(target_os = "macos")]
fn order_popover_front(window: &WebviewWindow, immediate_focus: bool) -> bool {
    let Ok(raw) = window.ns_window() else {
        return false;
    };
    if raw.is_null() {
        return false;
    }
    let ns = raw as *mut objc2::runtime::AnyObject;
    let ns_bits = ns as usize;
    unsafe {
        // NSPopUpMenuWindowLevel. Above normal windows, still under the menu bar.
        let _: () = objc2::msg_send![ns, setLevel: 101isize];
        let _: () = objc2::msg_send![ns, setAcceptsMouseMovedEvents: objc2::runtime::Bool::YES];
        let _: () = objc2::msg_send![ns, orderFrontRegardless];
    }
    if immediate_focus {
        focus_popover(ns);
    }
    // The status-item mouse-up resigns key if we activate during mouse-down,
    // which leaves the popover visible but without hover. Become key after
    // that click finishes. The keyboard roster asks for focus immediately
    // because there is no mouse-up coming.
    dispatch2::DispatchQueue::main().exec_async(move || {
        focus_popover(ns_bits as *mut objc2::runtime::AnyObject);
    });
    true
}

#[cfg(target_os = "macos")]
fn focus_popover(ns: *mut objc2::runtime::AnyObject) {
    if ns.is_null() {
        return;
    }
    let Some(app_cls) = objc2::runtime::AnyClass::get("NSApplication") else {
        return;
    };
    unsafe {
        let visible: objc2::runtime::Bool = objc2::msg_send![ns, isVisible];
        if !visible.as_bool() {
            return;
        }
        let _: () = objc2::msg_send![ns, makeKeyAndOrderFront: std::ptr::null_mut::<objc2::runtime::AnyObject>()];
        let app: *mut objc2::runtime::AnyObject = objc2::msg_send![app_cls, sharedApplication];
        if app.is_null() {
            return;
        }
        let _: () = objc2::msg_send![app, activateIgnoringOtherApps: objc2::runtime::Bool::YES];
    }
}

fn show_popover(app: &AppHandle, anchor: Option<&tauri::Rect>) {
    show_popover_inner(app, anchor, false);
}

fn show_keyboard_roster(app: &AppHandle) {
    KEYBOARD_ROSTER.store(true, Ordering::Relaxed);
    show_popover_inner(app, None, true);
    let _ = app.emit("keyboard-roster", true);
}

fn show_popover_inner(app: &AppHandle, anchor: Option<&tauri::Rect>, immediate_focus: bool) {
    if let Some(anchor) = anchor_from(app, anchor) {
        if let Ok(mut slot) = POPOVER_ANCHOR.lock() {
            *slot = Some(anchor);
        }
    }
    #[cfg(target_os = "macos")]
    remember_menu_bar_cursor();
    if let Some(window) = app.get_webview_window("main") {
        keep_off_taskbar(&window);
        let _ = window.unminimize();
        position_popover(app, &window);
        present_popover(&window, immediate_focus);
    }
    #[cfg(target_os = "macos")]
    arm_popover_dismiss(app);
}

fn main_is_visible(app: &AppHandle) -> bool {
    app.get_webview_window("main")
        .and_then(|window| window.is_visible().ok())
        .unwrap_or(false)
}

/// Hide the menu-bar popover only. Overlay glow windows and an in-progress tap stay up.
fn hide_popover(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
    }
    note_popover_hidden(app);
    #[cfg(target_os = "macos")]
    disarm_popover_monitors();
}

fn note_popover_hidden(app: &AppHandle) {
    KEYBOARD_ROSTER.store(false, Ordering::Relaxed);
    let _ = app.emit("popover-hidden", ());
}

fn toggle_popover(app: &AppHandle, anchor: Option<&tauri::Rect>) {
    if main_is_visible(app) {
        hide_popover(app);
    } else {
        show_popover(app, anchor);
    }
}

#[derive(Clone, Copy)]
struct AnchorPoint {
    x: f64,
    y: f64,
}

static POPOVER_ANCHOR: std::sync::Mutex<Option<AnchorPoint>> = std::sync::Mutex::new(None);

fn anchor_from(app: &AppHandle, anchor: Option<&tauri::Rect>) -> Option<AnchorPoint> {
    if let Some(rect) = anchor {
        let scale = app
            .get_webview_window("main")
            .and_then(|window| window.scale_factor().ok())
            .unwrap_or(1.0);
        let pos = rect.position.to_physical::<f64>(scale);
        let size = rect.size.to_physical::<f64>(scale);
        return Some(AnchorPoint {
            x: pos.x + size.width / 2.0,
            y: pos.y + size.height,
        });
    }
    if let Ok(cursor) = app.cursor_position() {
        return Some(AnchorPoint {
            x: cursor.x,
            y: cursor.y,
        });
    }
    None
}

/// Menu-bar / tray image.
///
/// On macOS the status item is drawn at 18pt from the 36px template (`@2x`),
/// black on transparent, with `icon_as_template` so it tracks the menu bar.
/// Other desktops get the color tile, which stays readable on light and dark panels.
fn tray_icon() -> tauri::image::Image<'static> {
    #[cfg(target_os = "macos")]
    const BYTES: &[u8] = include_bytes!("../icons/tray-icon@2x.png");
    #[cfg(not(target_os = "macos"))]
    const BYTES: &[u8] = include_bytes!("../icons/tray-icon-linux.png");
    tauri::image::Image::from_bytes(BYTES).expect("failed to load tray icon")
}

static POPOVER_REQUESTED: std::sync::Mutex<Option<(f64, f64)>> = std::sync::Mutex::new(None);

/// Point in AppKit space: origin at the bottom-left of the primary display, y upward.
#[cfg(target_os = "macos")]
#[repr(C)]
#[derive(Clone, Copy)]
struct MacPoint {
    x: f64,
    y: f64,
}

#[cfg(target_os = "macos")]
#[repr(C)]
#[derive(Clone, Copy)]
struct MacSize {
    width: f64,
    height: f64,
}

#[cfg(target_os = "macos")]
#[repr(C)]
#[derive(Clone, Copy)]
struct MacRect {
    origin: MacPoint,
    size: MacSize,
}

#[cfg(target_os = "macos")]
unsafe impl objc2::encode::Encode for MacPoint {
    const ENCODING: objc2::encode::Encoding = objc2::encode::Encoding::Struct(
        "CGPoint",
        &[
            <f64 as objc2::encode::Encode>::ENCODING,
            <f64 as objc2::encode::Encode>::ENCODING,
        ],
    );
}

#[cfg(target_os = "macos")]
unsafe impl objc2::encode::RefEncode for MacPoint {
    const ENCODING_REF: objc2::encode::Encoding =
        objc2::encode::Encoding::Pointer(&<Self as objc2::encode::Encode>::ENCODING);
}

#[cfg(target_os = "macos")]
unsafe impl objc2::encode::Encode for MacSize {
    const ENCODING: objc2::encode::Encoding = objc2::encode::Encoding::Struct(
        "CGSize",
        &[
            <f64 as objc2::encode::Encode>::ENCODING,
            <f64 as objc2::encode::Encode>::ENCODING,
        ],
    );
}

#[cfg(target_os = "macos")]
unsafe impl objc2::encode::RefEncode for MacSize {
    const ENCODING_REF: objc2::encode::Encoding =
        objc2::encode::Encoding::Pointer(&<Self as objc2::encode::Encode>::ENCODING);
}

#[cfg(target_os = "macos")]
unsafe impl objc2::encode::Encode for MacRect {
    const ENCODING: objc2::encode::Encoding = objc2::encode::Encoding::Struct(
        "CGRect",
        &[
            <MacPoint as objc2::encode::Encode>::ENCODING,
            <MacSize as objc2::encode::Encode>::ENCODING,
        ],
    );
}

#[cfg(target_os = "macos")]
unsafe impl objc2::encode::RefEncode for MacRect {
    const ENCODING_REF: objc2::encode::Encoding =
        objc2::encode::Encoding::Pointer(&<Self as objc2::encode::Encode>::ENCODING);
}

#[cfg(target_os = "macos")]
static MAC_CURSOR: std::sync::Mutex<Option<MacPoint>> = std::sync::Mutex::new(None);

#[cfg(target_os = "macos")]
fn macos_mouse() -> Option<MacPoint> {
    let cls = objc2::runtime::AnyClass::get("NSEvent")?;
    let point: MacPoint = unsafe { objc2::msg_send![cls, mouseLocation] };
    Some(point)
}

#[cfg(target_os = "macos")]
fn remember_menu_bar_cursor() {
    if let Some(point) = macos_mouse() {
        if let Ok(mut slot) = MAC_CURSOR.lock() {
            *slot = Some(point);
        }
    }
}

#[cfg(target_os = "macos")]
/// Block argument encoded as `@?` so objc2's debug checker accepts it.
#[repr(transparent)]
#[derive(Clone, Copy)]
struct ObjcBlock(*mut std::ffi::c_void);

#[cfg(target_os = "macos")]
unsafe impl objc2::encode::Encode for ObjcBlock {
    const ENCODING: objc2::encode::Encoding = objc2::encode::Encoding::Block;
}

#[cfg(target_os = "macos")]
unsafe impl objc2::encode::RefEncode for ObjcBlock {
    const ENCODING_REF: objc2::encode::Encoding = objc2::encode::Encoding::Block;
}

#[cfg(target_os = "macos")]
struct DismissBlocks {
    /// Leaked on purpose. `removeMonitor:` can race the callback that is
    /// currently on the stack; freeing the block there would be a use-after-free.
    local: *mut std::ffi::c_void,
    global: *mut std::ffi::c_void,
}

#[cfg(target_os = "macos")]
unsafe impl Send for DismissBlocks {}
#[cfg(target_os = "macos")]
unsafe impl Sync for DismissBlocks {}

#[cfg(target_os = "macos")]
struct MonitorTokens {
    local: *mut objc2::runtime::AnyObject,
    global: *mut objc2::runtime::AnyObject,
}

#[cfg(target_os = "macos")]
unsafe impl Send for MonitorTokens {}
#[cfg(target_os = "macos")]
unsafe impl Sync for MonitorTokens {}

#[cfg(target_os = "macos")]
static DISMISS_BLOCKS: std::sync::OnceLock<DismissBlocks> = std::sync::OnceLock::new();
#[cfg(target_os = "macos")]
static POPOVER_MONITORS: std::sync::Mutex<Option<MonitorTokens>> = std::sync::Mutex::new(None);

#[cfg(target_os = "macos")]
fn dismiss_blocks(app: &AppHandle) -> &'static DismissBlocks {
    DISMISS_BLOCKS.get_or_init(|| {
        let app_local = app.clone();
        let local = block2::RcBlock::new(
            move |event: *mut std::ffi::c_void| -> *mut std::ffi::c_void {
                on_popover_event(&app_local, event.cast(), true);
                event
            },
        );
        let app_global = app.clone();
        let global = block2::RcBlock::new(move |event: *mut std::ffi::c_void| {
            on_popover_event(&app_global, event.cast(), false);
        });
        DismissBlocks {
            local: block2::RcBlock::into_raw(local).cast(),
            global: block2::RcBlock::into_raw(global).cast(),
        }
    })
}

#[cfg(target_os = "macos")]
fn retain_returned(ptr: *mut objc2::runtime::AnyObject) -> *mut objc2::runtime::AnyObject {
    if ptr.is_null() {
        return std::ptr::null_mut();
    }
    unsafe {
        objc2::rc::Retained::retain(ptr)
            .map(objc2::rc::Retained::into_raw)
            .unwrap_or(std::ptr::null_mut())
    }
}

#[cfg(target_os = "macos")]
fn add_event_monitor(
    selector_global: bool,
    mask: usize,
    block: *mut std::ffi::c_void,
) -> *mut objc2::runtime::AnyObject {
    if block.is_null() {
        return std::ptr::null_mut();
    }
    let Some(cls) = objc2::runtime::AnyClass::get("NSEvent") else {
        return std::ptr::null_mut();
    };
    let handler = ObjcBlock(block);
    let created: *mut objc2::runtime::AnyObject = unsafe {
        if selector_global {
            objc2::msg_send![cls, addGlobalMonitorForEventsMatchingMask: mask, handler: handler]
        } else {
            objc2::msg_send![cls, addLocalMonitorForEventsMatchingMask: mask, handler: handler]
        }
    };
    retain_returned(created)
}

#[cfg(target_os = "macos")]
fn arm_popover_dismiss(app: &AppHandle) {
    let mut slot = match POPOVER_MONITORS.lock() {
        Ok(slot) => slot,
        Err(_) => return,
    };
    if slot.is_some() {
        return;
    }
    let blocks = dismiss_blocks(app);
    // NSEventMask is NSUInteger ('Q'). Left button 1, right button 3, key down 10.
    let mouse_mask: usize = (1 << 1) | (1 << 3);
    let local_mask: usize = mouse_mask | (1 << 10);
    let local = add_event_monitor(false, local_mask, blocks.local);
    if local.is_null() {
        return;
    }
    let global = add_event_monitor(true, mouse_mask, blocks.global);
    *slot = Some(MonitorTokens { local, global });
}

#[cfg(target_os = "macos")]
fn disarm_popover_monitors() {
    let tokens = match POPOVER_MONITORS.lock() {
        Ok(mut slot) => slot.take(),
        Err(_) => None,
    };
    let Some(tokens) = tokens else {
        return;
    };
    release_monitor(tokens.local);
    release_monitor(tokens.global);
}

#[cfg(target_os = "macos")]
fn release_monitor(token: *mut objc2::runtime::AnyObject) {
    if token.is_null() {
        return;
    }
    unsafe {
        if let Some(cls) = objc2::runtime::AnyClass::get("NSEvent") {
            let _: () = objc2::msg_send![cls, removeMonitor: token];
        }
        drop(objc2::rc::Retained::from_raw(token));
    }
}

#[cfg(target_os = "macos")]
fn dismiss_popover_from_monitor(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
    }
    note_popover_hidden(app);
    let app = app.clone();
    // Unregister after this callback returns. Removing the monitor that is
    // currently invoking us can free it out from under AppKit.
    dispatch2::DispatchQueue::main().exec_async(move || {
        if !main_is_visible(&app) {
            disarm_popover_monitors();
        }
    });
}

#[cfg(target_os = "macos")]
fn on_popover_event(app: &AppHandle, event: *mut objc2::runtime::AnyObject, include_keys: bool) {
    if event.is_null() {
        return;
    }
    if !main_is_visible(app) {
        // hidesOnDeactivate can order the panel out without going through hide_popover.
        let app = app.clone();
        dispatch2::DispatchQueue::main().exec_async(move || {
            if !main_is_visible(&app) {
                disarm_popover_monitors();
            }
        });
        return;
    }
    // NSEventType is NSUInteger. Left mouse down = 1, right = 3, key down = 10.
    // Selector is `type`, a Rust keyword, so it cannot go through `msg_send!`
    // (`stringify!(r#type)` is the literal `r#type`).
    let kind: usize = unsafe {
        use objc2::runtime::MessageReceiver;
        event.send_message(objc2::runtime::Sel::register("type"), ())
    };
    if include_keys && kind == 10 {
        let code: u16 = unsafe { objc2::msg_send![event, keyCode] };
        if code == 53 {
            dismiss_popover_from_monitor(app);
        }
        return;
    }
    if kind != 1 && kind != 3 {
        return;
    }
    if click_is_inside_popover(app, event) {
        return;
    }
    // Left click on the status item is the toggle. Hiding here would race the
    // tray handler and reopen the panel. Right click still dismisses.
    if kind == 1 && is_tray_button_window(event_window(event)) {
        return;
    }
    dismiss_popover_from_monitor(app);
}

#[cfg(target_os = "macos")]
fn event_window(event: *mut objc2::runtime::AnyObject) -> *mut objc2::runtime::AnyObject {
    unsafe { objc2::msg_send![event, window] }
}

#[cfg(target_os = "macos")]
fn click_is_inside_popover(app: &AppHandle, event: *mut objc2::runtime::AnyObject) -> bool {
    let Some(window) = app.get_webview_window("main") else {
        return false;
    };
    let Ok(raw) = window.ns_window() else {
        return false;
    };
    if raw.is_null() {
        return false;
    }
    let ns = raw as *mut objc2::runtime::AnyObject;
    if event_window(event) == ns {
        return true;
    }
    let Some(cursor) = macos_mouse() else {
        return false;
    };
    let frame: MacRect = unsafe { objc2::msg_send![ns, frame] };
    rect_contains(
        cursor.x,
        cursor.y,
        frame.origin.x,
        frame.origin.y,
        frame.size.width,
        frame.size.height,
    )
}

#[cfg(target_os = "macos")]
fn is_tray_button_window(window: *mut objc2::runtime::AnyObject) -> bool {
    if window.is_null() {
        return false;
    }
    // This process only owns the Tap status item. Its window is NSStatusBarWindow.
    // A full-width menu-bar window does not use that class, so clicks on the
    // Apple menu still count as outside.
    if class_chain_contains(window, b"StatusBar") {
        return true;
    }
    let frame: MacRect = unsafe { objc2::msg_send![window, frame] };
    let small = frame.size.height > 0.0
        && frame.size.height <= 80.0
        && frame.size.width > 0.0
        && frame.size.width <= 240.0;
    small && frame_touches_menu_bar(frame)
}

#[cfg(target_os = "macos")]
fn class_chain_contains(object: *mut objc2::runtime::AnyObject, needle: &[u8]) -> bool {
    unsafe {
        let mut class = objc2::ffi::object_getClass(object.cast());
        for _ in 0..16 {
            if class.is_null() {
                return false;
            }
            let name_ptr = objc2::ffi::class_getName(class);
            if !name_ptr.is_null() {
                let name = std::ffi::CStr::from_ptr(name_ptr);
                if name
                    .to_bytes()
                    .windows(needle.len())
                    .any(|chunk| chunk == needle)
                {
                    return true;
                }
            }
            class = objc2::ffi::class_getSuperclass(class);
        }
        false
    }
}

#[cfg(target_os = "macos")]
fn frame_touches_menu_bar(frame: MacRect) -> bool {
    let Some(screen_cls) = objc2::runtime::AnyClass::get("NSScreen") else {
        return false;
    };
    unsafe {
        let screens: *mut objc2::runtime::AnyObject = objc2::msg_send![screen_cls, screens];
        if screens.is_null() {
            return false;
        }
        // Swift screen arrays take NSInteger ('q') for count and objectAtIndex:.
        let count: isize = objc2::msg_send![screens, count];
        let top = frame.origin.y + frame.size.height;
        for index in 0..count {
            let screen: *mut objc2::runtime::AnyObject =
                objc2::msg_send![screens, objectAtIndex: index];
            if screen.is_null() {
                continue;
            }
            let screen_frame: MacRect = objc2::msg_send![screen, frame];
            let screen_top = screen_frame.origin.y + screen_frame.size.height;
            let along_top = (screen_top - top).abs() <= 4.0
                && frame.origin.x >= screen_frame.origin.x - 1.0
                && frame.origin.x < screen_frame.origin.x + screen_frame.size.width;
            if along_top {
                return true;
            }
        }
        false
    }
}

#[cfg(target_os = "macos")]
fn configure_popover_window(window: &WebviewWindow) {
    let Ok(raw) = window.ns_window() else {
        return;
    };
    if raw.is_null() {
        return;
    }
    let ns = raw as *mut objc2::runtime::AnyObject;
    unsafe {
        // A previous build set NSWindowStyleMaskNonactivatingPanel. That
        // keeps the popover from becoming key, so hover never starts.
        let mask: usize = objc2::msg_send![ns, styleMask];
        let nonactivating: usize = 1 << 7;
        if mask & nonactivating != 0 {
            let _: () = objc2::msg_send![ns, setStyleMask: mask & !nonactivating];
        }
        // MoveToActiveSpace | Stationary | IgnoresCycle | FullScreenAuxiliary
        // | CanJoinAllApplications. Joins the current app's space instead of
        // pulling Tap's other windows forward.
        let behavior: usize = (1 << 1) | (1 << 4) | (1 << 6) | (1 << 8) | (1 << 18);
        let _: () = objc2::msg_send![ns, setCollectionBehavior: behavior];
        // The panel is shown while Tap is inactive. Hiding on deactivate
        // would order it out immediately.
        let _: () = objc2::msg_send![ns, setHidesOnDeactivate: objc2::runtime::Bool::NO];
    }
}

#[cfg(target_os = "macos")]
fn install_resign_observer(app: AppHandle) {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let Some(center_cls) = objc2::runtime::AnyClass::get("NSNotificationCenter") else {
            return;
        };
        let center: *mut objc2::runtime::AnyObject =
            unsafe { objc2::msg_send![center_cls, defaultCenter] };
        if center.is_null() {
            return;
        }
        let name = ns_utf8_string("NSApplicationDidResignActiveNotification");
        if name.is_null() {
            return;
        }
        let block = block2::RcBlock::new(move |_note: *mut std::ffi::c_void| {
            let app = app.clone();
            dispatch2::DispatchQueue::main().exec_async(move || {
                if !main_is_visible(&app) {
                    disarm_popover_monitors();
                }
            });
        });
        let raw = block2::RcBlock::into_raw(block);
        let observer: *mut objc2::runtime::AnyObject = unsafe {
            objc2::msg_send![
                center,
                addObserverForName: name,
                object: std::ptr::null_mut::<objc2::runtime::AnyObject>(),
                queue: std::ptr::null_mut::<objc2::runtime::AnyObject>(),
                usingBlock: ObjcBlock(raw.cast())
            ]
        };
        // Leak the observer and the block for the life of the process.
        if !observer.is_null() {
            let _kept = retain_returned(observer);
        }
    });
}

#[cfg(target_os = "macos")]
fn ns_utf8_string(text: &str) -> *mut objc2::runtime::AnyObject {
    let Some(cls) = objc2::runtime::AnyClass::get("NSString") else {
        return std::ptr::null_mut();
    };
    let Ok(c) = std::ffi::CString::new(text) else {
        return std::ptr::null_mut();
    };
    unsafe { objc2::msg_send![cls, stringWithUTF8String: c.as_ptr()] }
}

/// Drop the popover under the menu bar of the display the cursor clicked.
///
/// Tao converts AppKit points with the main display's pixel height, then
/// `set_position` interprets them with whichever screen the window is already
/// on. On a second monitor that lands the window on the wrong display.
/// AppKit's own coordinates stay on the screen that contains the cursor.
#[cfg(target_os = "macos")]
fn place_popover_under_menu_bar(window: &WebviewWindow, width: f64, height: f64) {
    let Some(cursor) = MAC_CURSOR
        .lock()
        .ok()
        .and_then(|slot| *slot)
        .or_else(macos_mouse)
    else {
        return;
    };
    let Ok(raw) = window.ns_window() else {
        return;
    };
    let Some(screen_cls) = objc2::runtime::AnyClass::get("NSScreen") else {
        return;
    };
    let ns_window = raw as *mut objc2::runtime::AnyObject;
    unsafe {
        let screens: *mut objc2::runtime::AnyObject = objc2::msg_send![screen_cls, screens];
        if screens.is_null() {
            return;
        }
        // macOS 27's screen list is a Swift array. `count` and `objectAtIndex:`
        // use NSInteger ('q'); usize encodes as 'Q' and the debug checker aborts.
        let count: isize = objc2::msg_send![screens, count];
        let mut chosen: *mut objc2::runtime::AnyObject = std::ptr::null_mut();
        for index in 0..count {
            let screen: *mut objc2::runtime::AnyObject =
                objc2::msg_send![screens, objectAtIndex: index];
            let frame: MacRect = objc2::msg_send![screen, frame];
            if cursor.x >= frame.origin.x
                && cursor.y >= frame.origin.y
                && cursor.x < frame.origin.x + frame.size.width
                && cursor.y < frame.origin.y + frame.size.height
            {
                chosen = screen;
                break;
            }
        }
        if chosen.is_null() && count > 0 {
            chosen = objc2::msg_send![screens, objectAtIndex: 0isize];
        }
        if chosen.is_null() {
            return;
        }
        let visible: MacRect = objc2::msg_send![chosen, visibleFrame];
        let max_x = (visible.origin.x + visible.size.width - width).max(visible.origin.x);
        let max_y = (visible.origin.y + visible.size.height - height).max(visible.origin.y);
        // visibleFrame's top edge sits just under the menu bar. AppKit y grows upward.
        let x = (cursor.x - width / 2.0).clamp(visible.origin.x, max_x);
        let y =
            (visible.origin.y + visible.size.height - height - 4.0).clamp(visible.origin.y, max_y);
        let frame = MacRect {
            origin: MacPoint { x, y },
            size: MacSize { width, height },
        };
        let _: () = objc2::msg_send![ns_window, setFrame: frame display: true];
    }
}

fn requested_popover_size() -> (f64, f64) {
    if let Ok(guard) = POPOVER_REQUESTED.lock() {
        if let Some(size) = *guard {
            return size;
        }
    }
    (368.0, 220.0)
}

fn position_popover(app: &AppHandle, window: &WebviewWindow) {
    let (logical_w, logical_h) = requested_popover_size();
    #[cfg(target_os = "macos")]
    {
        let _ = app;
        place_popover_under_menu_bar(window, logical_w, logical_h);
        return;
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = window.set_size(tauri::LogicalSize::new(logical_w, logical_h));
        let anchor = match POPOVER_ANCHOR.lock() {
            Ok(slot) => *slot,
            Err(_) => None,
        };
        let Some(anchor) = anchor else {
            return;
        };
        let scale = window.scale_factor().unwrap_or(1.0);
        let win_w = logical_w * scale;
        let win_h = logical_h * scale;
        let Some((area_x, area_y, area_w, area_h)) = work_area_for_point(app, anchor.x, anchor.y)
        else {
            return;
        };
        let (x, y) = place_popover(
            anchor.x, anchor.y, win_w, win_h, area_x, area_y, area_w, area_h,
        );
        let _ = window.set_position(tauri::Position::Physical(tauri::PhysicalPosition {
            x: x.round() as i32,
            y: y.round() as i32,
        }));
    }
}

/// Place the popover just below the anchor, then keep the whole window inside the work area.
fn place_popover(
    anchor_x: f64,
    anchor_y: f64,
    win_w: f64,
    win_h: f64,
    area_x: f64,
    area_y: f64,
    area_w: f64,
    area_h: f64,
) -> (f64, f64) {
    let max_x = (area_x + area_w - win_w).max(area_x);
    let max_y = (area_y + area_h - win_h).max(area_y);
    let x = anchor_x - win_w / 2.0;
    let mut y = anchor_y + 8.0;
    if y > max_y {
        let above = anchor_y - win_h - 8.0;
        if above >= area_y {
            y = above;
        }
    }
    (x.clamp(area_x, max_x), y.clamp(area_y, max_y))
}

fn work_area_for_point(app: &AppHandle, x: f64, y: f64) -> Option<(f64, f64, f64, f64)> {
    let monitors = app.available_monitors().ok()?;
    let chosen = monitors
        .iter()
        .find(|monitor| contains_point(monitor, x, y))
        .or_else(|| monitors.first());
    let monitor = chosen?;
    let area = monitor.work_area();
    if area.size.width > 0 && area.size.height > 0 {
        Some((
            area.position.x as f64,
            area.position.y as f64,
            area.size.width as f64,
            area.size.height as f64,
        ))
    } else {
        let origin = monitor.position();
        let size = monitor.size();
        Some((
            origin.x as f64,
            origin.y as f64,
            size.width as f64,
            size.height as f64,
        ))
    }
}

fn rect_contains(px: f64, py: f64, x: f64, y: f64, w: f64, h: f64) -> bool {
    px >= x && py >= y && px < x + w && py < y + h
}

fn contains_point(monitor: &tauri::Monitor, x: f64, y: f64) -> bool {
    let origin = monitor.position();
    let size = monitor.size();
    let left = origin.x as f64;
    let top = origin.y as f64;
    x >= left && y >= top && x < left + size.width as f64 && y < top + size.height as f64
}

#[tauri::command]
fn reposition_popover(app: AppHandle, width: f64, height: f64) {
    if let Ok(mut slot) = POPOVER_REQUESTED.lock() {
        *slot = Some((width, height));
    }
    if let Some(window) = app.get_webview_window("main") {
        position_popover(&app, &window);
    }
}

#[tauri::command]
fn hide_settings(app: AppHandle) -> Result<(), String> {
    let Some(window) = app.get_webview_window("settings") else {
        return Err("Settings window is not available.".into());
    };
    window.hide().map_err(|e| e.to_string())
}

#[tauri::command]
fn dismiss_popover(app: AppHandle) {
    hide_popover(&app);
}

#[tauri::command]
fn consume_keyboard_open() -> bool {
    KEYBOARD_ROSTER.swap(false, Ordering::Relaxed)
}

#[cfg(target_os = "linux")]
static ROSTER_SHORTCUT: Mutex<String> = Mutex::new(String::new());
#[cfg(target_os = "linux")]
static ROSTER_SHORTCUT_ERROR: Mutex<String> = Mutex::new(String::new());

#[cfg(target_os = "linux")]
const GNOME_SHORTCUT_PATH: &str =
    "/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/tap-team/";

#[tauri::command]
fn set_roster_shortcut(
    state: State<AppState>,
    shortcut: String,
    app: AppHandle,
) -> Result<Settings, String> {
    let normalized = settings::normalize_roster_shortcut(&shortcut)?;
    #[cfg(target_os = "linux")]
    {
        // Registering waits for the main thread. Do it off this command so a
        // main-thread command cannot deadlock on itself.
        let (tx, rx) = std::sync::mpsc::channel();
        let registered = normalized.clone();
        let background = app.clone();
        std::thread::spawn(move || {
            let _ = tx.send(apply_roster_shortcut(&background, &registered));
        });
        rx.recv()
            .map_err(|_| "Couldn't register that shortcut.".to_string())??;
        let saved = {
            let mut cfg = state.settings.lock();
            cfg.roster_shortcut = Some(normalized);
            save_settings(&cfg)?;
            cfg.clone()
        };
        let _ = app.emit("settings-changed", saved.clone());
        return Ok(saved);
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (state, app, normalized);
        Err("On macOS, set this shortcut on the Raycast command.".into())
    }
}

#[tauri::command]
fn roster_shortcut_error() -> String {
    #[cfg(target_os = "linux")]
    {
        return ROSTER_SHORTCUT_ERROR.lock().clone();
    }
    #[cfg(not(target_os = "linux"))]
    String::new()
}

#[cfg(target_os = "linux")]
fn session_is_wayland() -> bool {
    std::env::var("XDG_SESSION_TYPE")
        .map(|value| value.eq_ignore_ascii_case("wayland"))
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn desktop_is_gnome() -> bool {
    std::env::var("XDG_CURRENT_DESKTOP")
        .map(|value| value.to_ascii_lowercase().contains("gnome"))
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn apply_roster_shortcut(app: &AppHandle, raw: &str) -> Result<(), String> {
    let normalized = settings::normalize_roster_shortcut(raw)?;
    if desktop_is_gnome() {
        match sync_gnome_shortcut(&settings::gnome_accelerator(&normalized)) {
            Ok(()) => {
                let _ = unregister_plugin_shortcut(app);
                *ROSTER_SHORTCUT.lock() = normalized;
                ROSTER_SHORTCUT_ERROR.lock().clear();
                return Ok(());
            }
            Err(err) => {
                if session_is_wayland() {
                    return remember_roster_error(format!(
                        "{err} Add a keyboard shortcut that runs: xdg-open tap://keys"
                    ));
                }
            }
        }
    } else if session_is_wayland() {
        return remember_roster_error(
            "This Wayland session can't give Tap a global shortcut. Add a keyboard shortcut that runs: xdg-open tap://keys"
                .to_string(),
        );
    }
    register_plugin_shortcut(app, &normalized)?;
    *ROSTER_SHORTCUT.lock() = normalized;
    ROSTER_SHORTCUT_ERROR.lock().clear();
    Ok(())
}

#[cfg(target_os = "linux")]
fn remember_roster_error(message: String) -> Result<(), String> {
    *ROSTER_SHORTCUT_ERROR.lock() = message.clone();
    Err(message)
}

#[cfg(target_os = "linux")]
fn register_plugin_shortcut(app: &AppHandle, normalized: &str) -> Result<(), String> {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;
    let gs = app.global_shortcut();
    let mut current = ROSTER_SHORTCUT.lock();
    if *current == normalized {
        return Ok(());
    }
    let previous = current.clone();
    if !previous.is_empty() {
        let _ = gs.unregister(previous.as_str());
    }
    if normalized.is_empty() {
        current.clear();
        return Ok(());
    }
    if let Err(err) = gs.register(normalized) {
        if !previous.is_empty() {
            let _ = gs.register(previous.as_str());
            *current = previous;
        }
        return remember_roster_error(format!("Couldn't register that shortcut ({err})."));
    }
    *current = normalized.to_string();
    Ok(())
}

#[cfg(target_os = "linux")]
fn unregister_plugin_shortcut(app: &AppHandle) -> Result<(), String> {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;
    let mut current = ROSTER_SHORTCUT.lock();
    if current.is_empty() {
        return Ok(());
    }
    let _ = app.global_shortcut().unregister(current.as_str());
    current.clear();
    Ok(())
}

#[cfg(target_os = "linux")]
fn sync_gnome_shortcut(accelerator: &str) -> Result<(), String> {
    let listed = gsettings(&[
        "get",
        "org.gnome.settings-daemon.plugins.media-keys",
        "custom-keybindings",
    ])?;
    let mut paths = gsettings_paths(&listed);
    let owned = paths.iter().any(|path| shortcut_paths_match(path, GNOME_SHORTCUT_PATH));
    if !owned {
        paths.push(GNOME_SHORTCUT_PATH.to_string());
        let list = format!(
            "[{}]",
            paths
                .iter()
                .map(|path| format!("'{path}'"))
                .collect::<Vec<_>>()
                .join(", ")
        );
        gsettings(&[
            "set",
            "org.gnome.settings-daemon.plugins.media-keys",
            "custom-keybindings",
            &list,
        ])?;
    }
    let schema = format!(
        "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:{GNOME_SHORTCUT_PATH}"
    );
    gsettings(&["set", &schema, "name", "'Open Tap team'"])?;
    gsettings(&["set", &schema, "command", "'xdg-open tap://keys'"])?;
    let binding = if accelerator.is_empty() {
        "''".to_string()
    } else {
        format!("'{accelerator}'")
    };
    gsettings(&["set", &schema, "binding", &binding])?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn shortcut_paths_match(left: &str, right: &str) -> bool {
    left.trim_end_matches('/') == right.trim_end_matches('/')
}

#[cfg(target_os = "linux")]
fn gsettings_paths(raw: &str) -> Vec<String> {
    let Some(start) = raw.find('[') else {
        return Vec::new();
    };
    let Some(end) = raw.rfind(']') else {
        return Vec::new();
    };
    raw[start + 1..end]
        .split(',')
        .filter_map(|part| {
            let path = part.trim().trim_matches('\'').trim();
            if path.is_empty() {
                None
            } else {
                Some(path.to_string())
            }
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn gsettings(args: &[&str]) -> Result<String, String> {
    let output = std::process::Command::new("gsettings")
        .args(args)
        .output()
        .map_err(|_| "gsettings isn't available.".to_string())?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        let err = err.trim();
        if err.is_empty() {
            return Err("Couldn't update the GNOME shortcut.".into());
        }
        return Err(err.to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn is_keyboard_link(raw: &str) -> bool {
    let raw = raw.trim().to_ascii_lowercase();
    let Some(rest) = raw
        .strip_prefix("tap://")
        .or_else(|| raw.strip_prefix("tap:"))
    else {
        return false;
    };
    let rest = rest.trim_start_matches('/');
    let token = rest.split(['/', '?', '#']).next().unwrap_or("");
    token == "keys"
}

fn open_urls(app: &AppHandle, urls: impl IntoIterator<Item = impl AsRef<str>>) {
    if urls.into_iter().any(|url| is_keyboard_link(url.as_ref())) {
        let shown = app.clone();
        let _ = app.run_on_main_thread(move || show_keyboard_roster(&shown));
    }
}

#[tauri::command]
fn open_settings(app: AppHandle) -> Result<(), String> {
    let Some(window) = app.get_webview_window("settings") else {
        return Err("Settings window is not available.".into());
    };
    window.unminimize().map_err(|e| e.to_string())?;
    let _ = window.set_always_on_top(false);
    // Windows created before the wider default stay narrow until relaunch.
    let scale = window.scale_factor().unwrap_or(1.0);
    if let Ok(outer) = window.outer_size() {
        let logical_w = outer.width as f64 / scale;
        if logical_w < 520.0 {
            let logical_h = outer.height as f64 / scale;
            let _ = window.set_size(tauri::Size::Logical(tauri::LogicalSize {
                width: 560.0,
                height: logical_h,
            }));
        }
    }
    window.show().map_err(|e| e.to_string())?;
    window.set_focus().map_err(|e| e.to_string())?;
    Ok(())
}

/// Hardware presence probe. Exits the process. See `status`.
#[cfg(target_os = "macos")]
pub fn run_status_probe() -> ! {
    status::run_probe();
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let settings_file_existed = settings_path().exists();
    let mut loaded = load_settings();
    let mut open_setup = !settings_file_existed;
    if loaded.team_key.trim().is_empty() {
        loaded.team_key = generate_team_key();
        let _ = save_settings(&loaded);
        open_setup = true;
    }
    #[cfg(target_os = "linux")]
    if loaded.roster_shortcut.is_none() {
        loaded.roster_shortcut = Some(settings::DEFAULT_ROSTER_SHORTCUT.into());
        let _ = save_settings(&loaded);
    }
    let settings = Arc::new(Mutex::new(loaded));
    let overlay = Arc::new(Mutex::new(OverlayStateMachine::default()));
    let peers = Arc::new(Mutex::new(HashMap::new()));

    let mut builder = tauri::Builder::default();
    // Single instance has to be registered first so a second `tap://` launch
    // is forwarded here instead of becoming another menu-bar app.
    builder = builder.plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
        open_urls(app, args);
    }));
    #[cfg(target_os = "linux")]
    {
        builder = builder.plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state == tauri_plugin_global_shortcut::ShortcutState::Pressed {
                        let shown = app.clone();
                        let _ = app.run_on_main_thread(move || show_keyboard_roster(&shown));
                    }
                })
                .build(),
        );
    }
    let mut app = builder
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--minimized"]),
        ))
        .manage(AppState {
            settings: settings.clone(),
            peers: peers.clone(),
            overlay: overlay.clone(),
            network: Mutex::new(None),
        })
        .setup(move |app| {
            apply_app_theme(app.handle(), &settings.lock().theme);
            let net = NetworkRuntime::start(
                app.handle().clone(),
                settings.clone(),
                overlay.clone(),
                peers.clone(),
            );
            app.state::<AppState>().network.lock().replace(net);

            let show = MenuItem::with_id(app, "show", "Open Tap", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;
            // macOS draws a template image at 18pt, so the 36px asset is the @2x.
            // Linux panels do not recolor templates; use the color tile instead.
            // Left click opens the popover. Right click keeps Open Tap / Quit.
            // A trayIcon in tauri.conf.json would add a second status item.
            let tray = TrayIconBuilder::with_id("main")
                .icon(tray_icon())
                .menu(&menu)
                .show_menu_on_left_click(false)
                .tooltip("Tap");
            #[cfg(target_os = "macos")]
            let tray = tray.icon_as_template(true);
            let _tray = tray
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => show_popover(app, None),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state,
                        rect,
                        ..
                    } = event
                    {
                        // macOS delivers both Down and Up. Toggle on Down so the
                        // outside-click monitor (also a mouse-down) cannot hide
                        // the panel and have the matching Up show it again.
                        // Other desktops report the activation on Up.
                        let activates = if cfg!(target_os = "macos") {
                            button_state == MouseButtonState::Down
                        } else {
                            button_state == MouseButtonState::Up
                        };
                        if activates {
                            toggle_popover(tray.app_handle(), Some(&rect));
                        }
                    }
                })
                .build(app)?;

            #[cfg(target_os = "macos")]
            if let Some(window) = app.get_webview_window("main") {
                let _ = window_vibrancy::apply_vibrancy(
                    &window,
                    window_vibrancy::NSVisualEffectMaterial::Popover,
                    None,
                    Some(22.0),
                );
                configure_popover_window(&window);
                install_resign_observer(app.handle().clone());
            }

            if open_setup {
                show_popover(app.handle(), None);
            } else if let Some(window) = app.get_webview_window("main") {
                let _ = window.hide();
            }

            #[cfg(any(target_os = "linux", all(debug_assertions, windows)))]
            {
                use tauri_plugin_deep_link::DeepLinkExt;
                let _ = app.deep_link().register_all();
            }
            {
                use tauri_plugin_deep_link::DeepLinkExt;
                let handle = app.handle().clone();
                if let Ok(Some(urls)) = app.deep_link().get_current() {
                    open_urls(&handle, urls.iter().map(|url| url.as_str().to_string()));
                }
                app.deep_link().on_open_url(move |event| {
                    open_urls(&handle, event.urls().iter().map(|url| url.to_string()));
                });
            }
            #[cfg(target_os = "linux")]
            {
                let handle = app.handle().clone();
                let raw = settings
                    .lock()
                    .roster_shortcut
                    .clone()
                    .unwrap_or_default();
                std::thread::spawn(move || {
                    let _ = apply_roster_shortcut(&handle, &raw);
                });
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                if window.label() == "main" {
                    hide_popover(window.app_handle());
                } else {
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            save_app_settings,
            open_settings,
            hide_settings,
            dismiss_popover,
            consume_keyboard_open,
            reposition_popover,
            list_peers,
            hold_start,
            hold_stop,
            preview_local,
            preview_glow,
            sender_id,
            dnd_state,
            set_do_not_disturb,
            set_peer_color,
            set_peer_key,
            set_roster_shortcut,
            roster_shortcut_error,
            remove_person,
            refresh_peers,
            discovery_refreshed_at,
            set_avatar,
            my_avatar,
            peer_avatar,
            list_displays,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    // Must be set before the event loop: tao otherwise launches as a regular app and
    // puts an icon in the Dock. Accessory keeps the status item and hides the Dock icon.
    #[cfg(target_os = "macos")]
    app.set_activation_policy(tauri::ActivationPolicy::Accessory);

    app.run(|app, event| {
        if let RunEvent::ExitRequested { .. } = event {
            if let Some(net) = app.state::<AppState>().network.lock().take() {
                net.stop();
            }
        }
    });
}

#[cfg(test)]
mod popover_placement_tests {
    use super::place_popover;

    #[test]
    fn outside_click_is_outside_the_popover_rect() {
        assert!(super::rect_contains(10.0, 10.0, 0.0, 0.0, 20.0, 20.0));
        assert!(!super::rect_contains(20.0, 10.0, 0.0, 0.0, 20.0, 20.0));
        assert!(!super::rect_contains(-0.5, 10.0, 0.0, 0.0, 20.0, 20.0));
    }

    #[test]
    fn keyboard_link_is_only_the_keys_roster() {
        assert!(super::is_keyboard_link("tap://keys"));
        assert!(super::is_keyboard_link("tap://keys/"));
        assert!(super::is_keyboard_link("  TAP://Keys?from=raycast"));
        assert!(!super::is_keyboard_link("tap://settings"));
        assert!(!super::is_keyboard_link("https://example.com/keys"));
    }

    #[test]
    fn clamps_popover_inside_1920_work_area_when_opened_from_the_tray() {
        // Panel occupies the top 29px. The tray click is near the right edge.
        let (x, y) = place_popover(1888.0, 14.0, 368.0, 400.0, 0.0, 29.0, 1920.0, 1171.0);
        assert!(x >= 0.0, "left {x}");
        assert!(x + 368.0 <= 1920.0 + 0.01, "right {}", x + 368.0);
        assert!(y >= 29.0, "top {y}");
        assert!(y + 400.0 <= 29.0 + 1171.0 + 0.01, "bottom {}", y + 400.0);
    }
}
