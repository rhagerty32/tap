mod discovery;
mod network;
mod overlay;
mod overlay_state;
mod peer;
mod protocol;
mod settings;

use crate::peer::{PeerInfo, PeerRecord};
use discovery::peer_list_from_map;
use network::{resolve_hold_addrs, stable_sender_id, NetworkRuntime};
use overlay_state::OverlayStateMachine;
use parking_lot::Mutex;
use settings::{generate_team_key, load_settings, save_settings, settings_path, Settings};
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
    save_settings(&settings)?;
    *state.settings.lock() = settings.clone();
    configure_autostart(&app, settings.launch_at_login)?;
    let _ = app.emit("settings-changed", settings);
    Ok(())
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
static TRAY_RECEIVING: AtomicBool = AtomicBool::new(false);

pub(crate) fn set_tray_sending(app: &AppHandle, sending: bool) {
    TRAY_SENDING.store(sending, Ordering::Relaxed);
    refresh_tray_icon(app);
}

pub(crate) fn set_tray_receiving(app: &AppHandle, receiving: bool) {
    TRAY_RECEIVING.store(receiving, Ordering::Relaxed);
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

fn show_popover(app: &AppHandle, anchor: Option<&tauri::Rect>) {
    if let Some(anchor) = anchor_from(app, anchor) {
        if let Ok(mut slot) = POPOVER_ANCHOR.lock() {
            *slot = Some(anchor);
        }
    }
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        position_popover(app, &window);
        let _ = window.set_focus();
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

fn position_popover(app: &AppHandle, window: &WebviewWindow) {
    let anchor = match POPOVER_ANCHOR.lock() {
        Ok(slot) => *slot,
        Err(_) => None,
    };
    let Some(anchor) = anchor else {
        return;
    };
    let scale = window.scale_factor().unwrap_or(1.0);
    let outer = window.outer_size().unwrap_or_default();
    let mut win_w = outer.width as f64;
    let mut win_h = outer.height as f64;
    if let Ok(guard) = POPOVER_REQUESTED.lock() {
        if let Some((logical_w, logical_h)) = *guard {
            win_w = win_w.max(logical_w * scale);
            win_h = win_h.max(logical_h * scale);
        }
    }
    if win_w < 2.0 {
        win_w = 368.0 * scale;
    }
    if win_h < 2.0 {
        win_h = 440.0 * scale;
    }
    let Some((area_x, area_y, area_w, area_h)) = work_area_for_point(app, anchor.x, anchor.y) else {
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
fn open_settings(app: AppHandle) -> Result<(), String> {
    let Some(window) = app.get_webview_window("settings") else {
        return Err("Settings window is not available.".into());
    };
    let outer = window.outer_size().unwrap_or_default();
    let scale = window.scale_factor().unwrap_or(1.0);
    let win_w = if outer.width > 1 {
        outer.width as f64
    } else {
        360.0 * scale
    };
    let win_h = if outer.height > 1 {
        outer.height as f64
    } else {
        540.0 * scale
    };
    let cursor = app.cursor_position().unwrap_or(tauri::PhysicalPosition { x: win_w, y: win_h });
    if let Some((area_x, area_y, area_w, area_h)) = work_area_for_point(&app, cursor.x, cursor.y) {
        let max_x = (area_x + area_w - win_w).max(area_x);
        let max_y = (area_y + area_h - win_h).max(area_y);
        let x = (area_x + (area_w - win_w) / 2.0).clamp(area_x, max_x);
        let y = (area_y + (area_h - win_h) / 2.0).clamp(area_y, max_y);
        let _ = window.set_position(tauri::Position::Physical(tauri::PhysicalPosition {
            x: x.round() as i32,
            y: y.round() as i32,
        }));
    }
    window.unminimize().map_err(|e| e.to_string())?;
    window.show().map_err(|e| e.to_string())?;
    window.set_focus().map_err(|e| e.to_string())?;
    Ok(())
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
    let settings = Arc::new(Mutex::new(loaded));
    let overlay = Arc::new(Mutex::new(OverlayStateMachine::default()));
    let peers = Arc::new(Mutex::new(HashMap::new()));

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--minimized"]),
        ))
        .manage(AppState {
            settings: settings.clone(),
            peers: peers.clone(),
            network: Mutex::new(None),
        })
        .setup(move |app| {
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
            let tray = TrayIconBuilder::with_id("main")
                .icon(tray_icon())
                .menu(&menu)
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
                        button_state: MouseButtonState::Up,
                        rect,
                        ..
                    } = event
                    {
                        show_popover(tray.app_handle(), Some(&rect));
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
            }

            if open_setup {
                show_popover(app.handle(), None);
            } else if let Some(window) = app.get_webview_window("main") {
                let _ = window.hide();
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            save_app_settings,
            open_settings,
            reposition_popover,
            list_peers,
            hold_start,
            hold_stop,
            preview_local,
            sender_id,
        ])
        .build(tauri::generate_context!())
        .expect("error while running tauri application")
        .run(|app, event| {
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
    fn clamps_popover_inside_1920_work_area_when_opened_from_the_tray() {
        // Panel occupies the top 29px. The tray click is near the right edge.
        let (x, y) = place_popover(1888.0, 14.0, 368.0, 400.0, 0.0, 29.0, 1920.0, 1171.0);
        assert!(x >= 0.0, "left {x}");
        assert!(x + 368.0 <= 1920.0 + 0.01, "right {}", x + 368.0);
        assert!(y >= 29.0, "top {y}");
        assert!(y + 400.0 <= 29.0 + 1171.0 + 0.01, "bottom {}", y + 400.0);
    }
}
