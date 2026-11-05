use crate::network::overlay_payload;
use crate::overlay_state::OverlaySnapshot;
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindowBuilder,
};

pub fn sync_overlay_windows(app: &AppHandle, snapshot: &OverlaySnapshot) {
    let app = app.clone();
    let snapshot = snapshot.clone();
    // AppKit window calls (setCollectionBehavior, show, hide) crash with
    // "Must only be used from the main thread" if the UDP receiver invokes them.
    let _ = app
        .clone()
        .run_on_main_thread(move || apply_overlay_windows(&app, &snapshot));
}

fn apply_overlay_windows(app: &AppHandle, snapshot: &OverlaySnapshot) {
    match snapshot {
        OverlaySnapshot::Hidden => {
            hide_all(app);
            crate::set_tray_receiving(app, false);
        }
        OverlaySnapshot::Visible(_) => {
            let payload = overlay_payload(snapshot.clone());
            crate::set_tray_receiving(app, true);
            let monitors = app.available_monitors().unwrap_or_default();
            if monitors.is_empty() {
                return;
            }
            for (idx, monitor) in monitors.iter().enumerate() {
                let label = format!("overlay-{idx}");
                let pos = monitor.position();
                let size = monitor.size();
                let scale = monitor.scale_factor();
                let url = WebviewUrl::App(format!("overlay.html?v={idx}").into());
                let window = if let Some(w) = app.get_webview_window(&label) {
                    w
                } else {
                    match WebviewWindowBuilder::new(app, &label, url)
                        .transparent(true)
                        .decorations(false)
                        .always_on_top(true)
                        .skip_taskbar(true)
                        .visible(false)
                        .focused(false)
                        .resizable(false)
                        .position(pos.x as f64 / scale, pos.y as f64 / scale)
                        .inner_size(size.width as f64 / scale, size.height as f64 / scale)
                        .build()
                    {
                        Ok(w) => w,
                        Err(_) => continue,
                    }
                };
                let _ = window.set_position(tauri::Position::Physical(PhysicalPosition {
                    x: pos.x,
                    y: pos.y,
                }));
                let _ = window.set_size(tauri::Size::Physical(PhysicalSize {
                    width: size.width,
                    height: size.height,
                }));
                let _ = window.show();
                // Click-through asks GDK for the realized window. Doing it before show
                // unwraps None inside tao and aborts the process, so the glow never appears.
                let _ = window.set_ignore_cursor_events(true);
                configure_macos_window(&window);
                let _ = window.emit("overlay-changed", payload.clone());
            }
            for idx in monitors.len()..8 {
                if let Some(w) = app.get_webview_window(&format!("overlay-{idx}")) {
                    let _ = w.hide();
                }
            }
        }
    }
}

fn hide_all(app: &AppHandle) {
    for idx in 0..8 {
        if let Some(w) = app.get_webview_window(&format!("overlay-{idx}")) {
            let _ = w.hide();
        }
    }
}

#[cfg(target_os = "macos")]
fn configure_macos_window(window: &tauri::WebviewWindow) {
    if let Ok(ns_window) = window.ns_window() {
        unsafe {
            use objc2::msg_send;
            use objc2::runtime::AnyObject;
            let window_ptr = ns_window as *mut AnyObject;
            if window_ptr.is_null() {
                return;
            }
            // NSWindowCollectionBehavior: CanJoinAllSpaces | FullScreenAuxiliary | IgnoresCycle
            let behavior: usize = (1 << 0) | (1 << 8) | (1 << 4);
            let _: () = msg_send![window_ptr, setCollectionBehavior: behavior];
            let _: () = msg_send![window_ptr, setIgnoresMouseEvents: true];
            let _: () = msg_send![window_ptr, setHasShadow: false];
            // NSScreenSaverWindowLevel, so the edge glow sits above normal windows.
            let level: i32 = 1000;
            let _: () = msg_send![window_ptr, setLevel: level];
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn configure_macos_window(_window: &tauri::WebviewWindow) {}
