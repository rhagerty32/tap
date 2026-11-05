mod discovery;
mod network;
mod overlay;
mod overlay_state;
mod peer;
mod protocol;
mod settings;

use discovery::peer_list_from_map;
use network::{resolve_hold_addrs, NetworkRuntime, stable_sender_id};
use overlay_state::OverlayStateMachine;
use parking_lot::Mutex;
use crate::peer::{PeerInfo, PeerRecord};
use settings::{generate_team_key, load_settings, save_settings, settings_path, Settings};
use std::collections::HashMap;
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
fn save_app_settings(state: State<AppState>, settings: Settings, app: AppHandle) -> Result<(), String> {
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
fn hold_start(state: State<AppState>, peer_id: String) -> Result<(), String> {
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
    Ok(())
}

#[tauri::command]
fn hold_stop(state: State<AppState>) {
    if let Some(net) = state.network.lock().as_ref() {
        net.end_hold();
    }
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

fn show_popover(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
        center_popover(app, &window);
    }
}

fn center_popover(app: &AppHandle, window: &WebviewWindow) {
    if let Ok(Some(monitor)) = app.primary_monitor() {
        let size = window.outer_size().unwrap_or_default();
        let mon_pos = monitor.position();
        let mon_size = monitor.size();
        let x = mon_pos.x + (mon_size.width as i32 - size.width as i32) / 2;
        let y = mon_pos.y + (mon_size.height as i32 - size.height as i32) / 2;
        let _ = window.set_position(tauri::Position::Physical(tauri::PhysicalPosition { x, y }));
    }
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
            let _tray = TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .menu(&menu)
                .tooltip("Tap")
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => show_popover(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        show_popover(tray.app_handle());
                    }
                })
                .build(app)?;

            if open_setup {
                show_popover(app.handle());
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
