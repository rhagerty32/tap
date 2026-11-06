use crate::overlay_state::OverlaySnapshot;
use crate::settings::{normalize_glow, normalize_screens, Settings};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
#[cfg(not(target_os = "macos"))]
use tauri::{PhysicalPosition, PhysicalSize};

/// Enough for a laptop plus a full dock of external displays.
const MAX_OVERLAYS: usize = 16;

/// Keep the window up through the CSS fade. `--glow-out` is 640ms; the page
/// finishes a little after that. Hiding sooner clips the exit.
const GLOW_HIDE_AFTER: std::time::Duration = std::time::Duration::from_millis(720);

static HIDE_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Latest glow payload. Page loads read this so a window that finishes
/// loading after the one-shot emit still paints.
static OVERLAY_PAYLOAD: Mutex<Option<serde_json::Value>> = Mutex::new(None);
static WINDOW_PAYLOADS: Mutex<Vec<serde_json::Value>> = Mutex::new(Vec::new());
static OVERLAY_CHROME: Mutex<Option<OverlayChrome>> = Mutex::new(None);

#[derive(Clone)]
pub struct OverlayChrome {
    pub glow_style: String,
    pub show_banner: bool,
    pub screens: String,
    pub selected: Vec<String>,
    pub avatar: Option<String>,
    pub chime: bool,
}

impl OverlayChrome {
    pub fn from_settings(settings: &Settings, avatar: Option<String>) -> Self {
        Self {
            glow_style: normalize_glow(&settings.glow_style),
            show_banner: settings.show_banner,
            screens: normalize_screens(&settings.overlay_screens),
            selected: settings.selected_displays.clone(),
            avatar,
            chime: settings.chime_enabled,
        }
    }
}

pub fn sync_overlay_windows(app: &AppHandle, snapshot: &OverlaySnapshot, chrome: OverlayChrome) {
    let app = app.clone();
    let snapshot = snapshot.clone();
    // AppKit window calls (setCollectionBehavior, orderFront, hide) crash with
    // "Must only be used from the main thread" if the UDP receiver invokes them.
    let _ = app
        .clone()
        .run_on_main_thread(move || apply_overlay_windows(&app, &snapshot, &chrome));
}

fn remember_payload(payload: &serde_json::Value) {
    if let Ok(mut slot) = OVERLAY_PAYLOAD.lock() {
        *slot = Some(payload.clone());
    }
}

fn remembered_payload() -> Option<serde_json::Value> {
    OVERLAY_PAYLOAD.lock().ok().and_then(|slot| slot.clone())
}

fn forget_payload() {
    if let Ok(mut slot) = OVERLAY_PAYLOAD.lock() {
        *slot = None;
    }
    if let Ok(mut slot) = WINDOW_PAYLOADS.lock() {
        slot.clear();
    }
    if let Ok(mut slot) = OVERLAY_CHROME.lock() {
        *slot = None;
    }
}

fn remember_chrome(chrome: &OverlayChrome) {
    if let Ok(mut slot) = OVERLAY_CHROME.lock() {
        *slot = Some(chrome.clone());
    }
}

fn remembered_chrome() -> OverlayChrome {
    OVERLAY_CHROME
        .lock()
        .ok()
        .and_then(|slot| slot.clone())
        .unwrap_or_else(|| OverlayChrome {
            glow_style: "edge".into(),
            show_banner: true,
            screens: "all".into(),
            selected: Vec::new(),
            avatar: None,
            chime: false,
        })
}

fn apply_overlay_windows(app: &AppHandle, snapshot: &OverlaySnapshot, chrome: &OverlayChrome) {
    match snapshot {
        OverlaySnapshot::Hidden => {
            crate::set_tray_receiving(app, false);
            let _ = take_shown();
            forget_payload();
            emit_hidden(app);
            // The page fades the glow. Hide the window only after that finishes.
            schedule_hide_all(app);
        }
        OverlaySnapshot::Visible(active) => {
            cancel_pending_hide();
            let payload = overlay_event_payload(active, chrome);
            remember_payload(&payload);
            remember_chrome(chrome);
            crate::set_tray_receiving(app, true);
            let count = show_on_each_display(app, &payload, chrome);
            if count == 0 {
                eprintln!("tap: no displays reported for the overlay");
            }
        }
    }
}

fn overlay_event_payload(
    active: &crate::overlay_state::ActiveOverlay,
    chrome: &OverlayChrome,
) -> serde_json::Value {
    serde_json::json!({
        "visible": true,
        "senderName": active.sender_name,
        "color": active.color,
        "glowStyle": chrome.glow_style,
        "showBanner": chrome.show_banner,
        "chimeEnabled": chrome.chime,
        "avatar": chrome.avatar,
    })
}

fn remember_window_payloads(payloads: &[serde_json::Value]) {
    if let Ok(mut slot) = WINDOW_PAYLOADS.lock() {
        *slot = payloads.to_vec();
    }
}

fn window_payload(index: usize) -> Option<serde_json::Value> {
    WINDOW_PAYLOADS
        .lock()
        .ok()
        .and_then(|slot| slot.get(index).cloned())
}

fn overlay_index(label: &str) -> Option<usize> {
    label.strip_prefix("overlay-")?.parse().ok()
}

fn show_on_each_display(
    app: &AppHandle,
    payload: &serde_json::Value,
    chrome: &OverlayChrome,
) -> usize {
    #[cfg(target_os = "macos")]
    {
        ensure_screen_watch(app);
        let screens = macos_display_frames();
        let unique = filter_mac_screens(unique_screens(&screens), chrome);
        if unique.is_empty() {
            hide_all(app);
            remember_frames(&unique);
            return 0;
        }
        if PLACING.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return unique.len();
        }
        let _placing = PlacingGuard;
        let payloads = screen_payloads(payload, unique.len());
        remember_window_payloads(&payloads);
        if frames_unchanged(&unique) {
            // Geometry already matches this screen set. Emit the payload in case
            // the sender or glow style changed, but do not setFrame again.
            emit_window_payloads(app);
            return unique.len();
        }
        log_screens(&screens);
        let mut shown = 0usize;
        for (idx, screen) in unique.iter().enumerate() {
            let Some(window) = overlay_window(
                app,
                idx,
                None,
                Some((screen.frame.size.width, screen.frame.size.height)),
            ) else {
                continue;
            };
            // Collection behavior and the target frame must be set before the
            // window is ordered in. `show()` is makeKeyAndOrderFront, which
            // puts the window on the active space first; a later setFrame is
            // then clamped to that space, and a transparent WKWebView often
            // never composites after the move. orderFrontRegardless is the
            // first time the window becomes visible, already on this screen.
            if present_macos_overlay(&window, idx, screen.frame) {
                shown += 1;
            }
            // Click-through via tao is async and must not run before the
            // window exists. AppKit already set ignoresMouseEvents.
            let _ = window.set_ignore_cursor_events(true);
            if let Some(window_payload) = payloads.get(idx) {
                let _ = window.emit("overlay-changed", window_payload.clone());
            }
        }
        for idx in unique.len()..MAX_OVERLAYS {
            if let Some(w) = app.get_webview_window(&format!("overlay-{idx}")) {
                let _ = w.hide();
            }
        }
        // Remember even a partial placement so a screen-parameter notification
        // caused by setFrame does not place and log again for the same frames.
        remember_frames(&unique);
        if shown == unique.len() {
            eprintln!("tap: glow on {shown} display(s)");
        } else {
            eprintln!("tap: glow on {shown} of {} display(s)", unique.len());
        }
        if shown > 0 {
            // The overlay page registers its listener after the first emit.
            // A couple of later emits cover that without touching geometry.
            emit_until_listening(app, payload);
        }
        shown
    }
    #[cfg(not(target_os = "macos"))]
    {
        let monitors = filter_linux_monitors(app, chrome);
        let count = monitors.len();
        if count == 0 {
            hide_all(app);
            return 0;
        }
        let payloads = screen_payloads(payload, count);
        remember_window_payloads(&payloads);
        for (idx, monitor) in monitors.iter().enumerate() {
            let pos = monitor.position();
            let size = monitor.size();
            let scale = monitor.scale_factor();
            let Some(window) = overlay_window(
                app,
                idx,
                Some((
                    pos.x as f64 / scale,
                    pos.y as f64 / scale,
                    size.width as f64 / scale,
                    size.height as f64 / scale,
                )),
                None,
            ) else {
                continue;
            };
            let _ = window.set_position(tauri::Position::Physical(PhysicalPosition {
                x: pos.x,
                y: pos.y,
            }));
            let _ = window.set_size(tauri::Size::Physical(PhysicalSize {
                width: size.width,
                height: size.height,
            }));
            let _ = window.set_skip_taskbar(true);
            let _ = window.show();
            let _ = window.set_ignore_cursor_events(true);
            if let Some(window_payload) = payloads.get(idx) {
                let _ = window.emit("overlay-changed", window_payload.clone());
            }
        }
        for idx in count..MAX_OVERLAYS {
            if let Some(window) = app.get_webview_window(&format!("overlay-{idx}")) {
                let _ = window.hide();
            }
        }
        if count > 0 {
            mark_shown();
        }
        count
    }
}

#[cfg(not(target_os = "macos"))]
fn linux_monitor_name(monitor: &tauri::Monitor, index: usize) -> String {
    monitor
        .name()
        .filter(|name| !name.is_empty())
        .cloned()
        .unwrap_or_else(|| format!("Display {}", index + 1))
}

#[cfg(not(target_os = "macos"))]
fn filter_linux_monitors(app: &AppHandle, chrome: &OverlayChrome) -> Vec<tauri::Monitor> {
    let monitors = app.available_monitors().unwrap_or_default();
    let named: Vec<_> = monitors
        .into_iter()
        .enumerate()
        .map(|(index, monitor)| (linux_monitor_name(&monitor, index), monitor))
        .collect();
    match chrome.screens.as_str() {
        "cursor" => {
            let Ok(cursor) = app.cursor_position() else {
                return named.into_iter().map(|(_, monitor)| monitor).collect();
            };
            let picked: Vec<_> = named
                .into_iter()
                .filter(|(_, monitor)| {
                    let pos = monitor.position();
                    let size = monitor.size();
                    cursor.x >= pos.x as f64
                        && cursor.y >= pos.y as f64
                        && cursor.x < pos.x as f64 + size.width as f64
                        && cursor.y < pos.y as f64 + size.height as f64
                })
                .map(|(_, monitor)| monitor)
                .collect();
            if picked.is_empty() {
                app.available_monitors().unwrap_or_default()
            } else {
                picked
            }
        }
        "selected" => named
            .into_iter()
            .filter(|(name, _)| chrome.selected.iter().any(|wanted| wanted == name))
            .map(|(_, monitor)| monitor)
            .collect(),
        _ => named.into_iter().map(|(_, monitor)| monitor).collect(),
    }
}

pub fn display_names(app: &AppHandle) -> Vec<String> {
    #[cfg(target_os = "macos")]
    {
        let _ = app;
        return macos_display_frames()
            .into_iter()
            .map(|screen| screen.name)
            .collect();
    }
    #[cfg(not(target_os = "macos"))]
    {
        app.available_monitors()
            .unwrap_or_default()
            .iter()
            .enumerate()
            .map(|(index, monitor)| linux_monitor_name(monitor, index))
            .collect()
    }
}

fn screen_payloads(base: &serde_json::Value, count: usize) -> Vec<serde_json::Value> {
    vec![base.clone(); count]
}

fn emit_hidden(app: &AppHandle) {
    let payload = serde_json::json!({ "visible": false });
    for idx in 0..MAX_OVERLAYS {
        if let Some(window) = app.get_webview_window(&format!("overlay-{idx}")) {
            let _ = window.emit("overlay-changed", payload.clone());
        }
    }
}

fn emit_window_payloads(app: &AppHandle) {
    let payloads = WINDOW_PAYLOADS
        .lock()
        .map(|slot| slot.clone())
        .unwrap_or_default();
    for (idx, payload) in payloads.iter().enumerate() {
        if let Some(window) = app.get_webview_window(&format!("overlay-{idx}")) {
            let _ = window.emit("overlay-changed", payload.clone());
        }
    }
}

fn emit_until_listening(app: &AppHandle, payload: &serde_json::Value) {
    let app = app.clone();
    let payload = payload.clone();
    std::thread::spawn(move || {
        for delay_ms in [120_u64, 450] {
            std::thread::sleep(std::time::Duration::from_millis(delay_ms));
            let app = app.clone();
            let payload = payload.clone();
            let app_main = app.clone();
            let _ = app.run_on_main_thread(move || {
                let Some(current) = remembered_payload() else {
                    return;
                };
                if current != payload {
                    return;
                }
                emit_window_payloads(&app_main);
            });
        }
    });
}

/// `geom` is logical x, y, width, height. macOS omits it and passes only a
/// point size so the webview is born at that display's size. Position is
/// left unset: tao would center the window, then an async `set_position`
/// would overwrite the AppKit frame.
fn overlay_window(
    app: &AppHandle,
    idx: usize,
    geom: Option<(f64, f64, f64, f64)>,
    logical_size: Option<(f64, f64)>,
) -> Option<tauri::WebviewWindow> {
    let label = format!("overlay-{idx}");
    if let Some(window) = app.get_webview_window(&label) {
        return Some(window);
    }
    let url = WebviewUrl::App(format!("overlay.html?v={idx}").into());
    let mut builder = WebviewWindowBuilder::new(app, &label, url)
        .transparent(true)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible(false)
        .focused(false)
        .resizable(false)
        .on_page_load(|window, load| {
            if load.event() != tauri::webview::PageLoadEvent::Finished {
                return;
            }
            let payload = overlay_index(window.label())
                .and_then(window_payload)
                .or_else(remembered_payload);
            let Some(payload) = payload else {
                return;
            };
            let _ = window.emit("overlay-changed", payload);
        });
    #[cfg(target_os = "macos")]
    {
        // CanJoinAllSpaces at creation, before the window is ever ordered in.
        builder = builder.visible_on_all_workspaces(true);
    }
    if let Some((x, y, width, height)) = geom {
        builder = builder.position(x, y).inner_size(width, height);
    } else if let Some((width, height)) = logical_size {
        builder = builder.inner_size(width, height);
    }
    match builder.build() {
        Ok(window) => Some(window),
        Err(err) => {
            eprintln!("tap: overlay window {label} failed: {err}");
            None
        }
    }
}

fn hide_all(app: &AppHandle) {
    for idx in 0..MAX_OVERLAYS {
        if let Some(w) = app.get_webview_window(&format!("overlay-{idx}")) {
            let _ = w.hide();
        }
    }
}

fn cancel_pending_hide() {
    HIDE_GENERATION.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
}

fn schedule_hide_all(app: &AppHandle) {
    let token = HIDE_GENERATION.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(GLOW_HIDE_AFTER);
        if HIDE_GENERATION.load(std::sync::atomic::Ordering::SeqCst) != token {
            return;
        }
        let app_for_hide = app.clone();
        let _ = app.run_on_main_thread(move || {
            if HIDE_GENERATION.load(std::sync::atomic::Ordering::SeqCst) != token {
                return;
            }
            hide_all(&app_for_hide);
        });
    });
}

/// AppKit point rect. Origin is the bottom-left of the primary display, y upward.
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
#[derive(Clone)]
struct ScreenInfo {
    index: isize,
    frame: MacRect,
    scale: f64,
    screen_id: u32,
    name: String,
}

#[cfg(target_os = "macos")]
static LAST_FRAMES: Mutex<Option<Vec<MacRect>>> = Mutex::new(None);

#[cfg(target_os = "macos")]
static PLACING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(target_os = "macos")]
struct PlacingGuard;

#[cfg(target_os = "macos")]
impl Drop for PlacingGuard {
    fn drop(&mut self) {
        PLACING.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

#[cfg(target_os = "macos")]
static OVERLAY_APP: std::sync::OnceLock<AppHandle> = std::sync::OnceLock::new();

#[cfg(not(target_os = "macos"))]
static SHOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn take_shown() -> bool {
    #[cfg(target_os = "macos")]
    {
        match LAST_FRAMES.lock() {
            Ok(mut slot) => slot.take().is_some(),
            Err(poisoned) => poisoned.into_inner().take().is_some(),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        SHOWN.swap(false, std::sync::atomic::Ordering::SeqCst)
    }
}

#[cfg(not(target_os = "macos"))]
fn mark_shown() {
    SHOWN.store(true, std::sync::atomic::Ordering::SeqCst);
}

#[cfg(target_os = "macos")]
fn frames_unchanged(unique: &[ScreenInfo]) -> bool {
    let Ok(slot) = LAST_FRAMES.lock() else {
        return false;
    };
    let Some(prev) = slot.as_ref() else {
        return false;
    };
    if prev.len() != unique.len() {
        return false;
    }
    prev.iter()
        .zip(unique.iter())
        .all(|(prev, screen)| rects_match(*prev, screen.frame))
}

#[cfg(target_os = "macos")]
fn remember_frames(unique: &[ScreenInfo]) {
    let frames = unique.iter().map(|screen| screen.frame).collect();
    match LAST_FRAMES.lock() {
        Ok(mut slot) => *slot = Some(frames),
        Err(poisoned) => *poisoned.into_inner() = Some(frames),
    }
}

#[cfg(target_os = "macos")]
fn rects_match(a: MacRect, b: MacRect) -> bool {
    const EPS: f64 = 1.0;
    (a.origin.x - b.origin.x).abs() <= EPS
        && (a.origin.y - b.origin.y).abs() <= EPS
        && (a.size.width - b.size.width).abs() <= EPS
        && (a.size.height - b.size.height).abs() <= EPS
}

#[cfg(target_os = "macos")]
fn macos_display_frames() -> Vec<ScreenInfo> {
    let Some(screen_cls) = objc2::runtime::AnyClass::get("NSScreen") else {
        return Vec::new();
    };
    unsafe {
        let screens: *mut objc2::runtime::AnyObject = objc2::msg_send![screen_cls, screens];
        if screens.is_null() {
            return Vec::new();
        }
        // Same Swift array as the popover path: count and the index are NSInteger.
        let count: isize = objc2::msg_send![screens, count];
        let limit = count.max(0).min(MAX_OVERLAYS as isize);
        let mut frames = Vec::with_capacity(limit as usize);
        for index in 0..limit {
            let screen: *mut objc2::runtime::AnyObject =
                objc2::msg_send![screens, objectAtIndex: index];
            if screen.is_null() {
                eprintln!("tap: screen {index} is null");
                continue;
            }
            let frame: MacRect = objc2::msg_send![screen, frame];
            let scale: f64 = objc2::msg_send![screen, backingScaleFactor];
            let screen_id = screen_number(screen);
            if frame.size.width < 1.0 || frame.size.height < 1.0 {
                eprintln!(
                    "tap: screen {index} id {screen_id} skipped, origin ({:.0}, {:.0}) size {:.0}x{:.0}",
                    frame.origin.x, frame.origin.y, frame.size.width, frame.size.height
                );
                continue;
            }
            frames.push(ScreenInfo {
                index,
                frame,
                scale,
                screen_id,
                name: screen_name(screen, index),
            });
        }
        frames
    }
}

#[cfg(target_os = "macos")]
fn screen_number(screen: *mut objc2::runtime::AnyObject) -> u32 {
    unsafe {
        let desc: *mut objc2::runtime::AnyObject = objc2::msg_send![screen, deviceDescription];
        if desc.is_null() {
            return 0;
        }
        let Some(str_cls) = objc2::runtime::AnyClass::get("NSString") else {
            return 0;
        };
        let key: *mut objc2::runtime::AnyObject =
            objc2::msg_send![str_cls, stringWithUTF8String: c"NSScreenNumber".as_ptr()];
        if key.is_null() {
            return 0;
        }
        let num: *mut objc2::runtime::AnyObject = objc2::msg_send![desc, objectForKey: key];
        if num.is_null() {
            return 0;
        }
        objc2::msg_send![num, unsignedIntValue]
    }
}

#[cfg(target_os = "macos")]
fn screen_name(screen: *mut objc2::runtime::AnyObject, index: isize) -> String {
    unsafe {
        let name: *mut objc2::runtime::AnyObject = objc2::msg_send![screen, localizedName];
        if !name.is_null() {
            let cstr: *const i8 = objc2::msg_send![name, UTF8String];
            if !cstr.is_null() {
                if let Ok(text) = std::ffi::CStr::from_ptr(cstr).to_str() {
                    if !text.is_empty() {
                        return text.to_string();
                    }
                }
            }
        }
    }
    format!("Display {}", index + 1)
}

#[cfg(target_os = "macos")]
fn macos_cursor() -> Option<MacPoint> {
    let cls = objc2::runtime::AnyClass::get("NSEvent")?;
    unsafe {
        let point: MacPoint = objc2::msg_send![cls, mouseLocation];
        Some(point)
    }
}

#[cfg(target_os = "macos")]
fn point_in_frame(point: MacPoint, frame: MacRect) -> bool {
    point.x >= frame.origin.x
        && point.y >= frame.origin.y
        && point.x < frame.origin.x + frame.size.width
        && point.y < frame.origin.y + frame.size.height
}

#[cfg(target_os = "macos")]
fn filter_mac_screens(screens: Vec<ScreenInfo>, chrome: &OverlayChrome) -> Vec<ScreenInfo> {
    match chrome.screens.as_str() {
        "cursor" => {
            let Some(cursor) = macos_cursor() else {
                return screens;
            };
            let picked: Vec<_> = screens
                .iter()
                .filter(|screen| point_in_frame(cursor, screen.frame))
                .cloned()
                .collect();
            if picked.is_empty() {
                screens
            } else {
                picked
            }
        }
        "selected" => screens
            .into_iter()
            .filter(|screen| chrome.selected.iter().any(|name| name == &screen.name))
            .collect(),
        _ => screens,
    }
}

#[cfg(target_os = "macos")]
fn unique_screens(screens: &[ScreenInfo]) -> Vec<ScreenInfo> {
    let mut unique = Vec::new();
    for screen in screens {
        if unique
            .iter()
            .any(|earlier: &ScreenInfo| rects_match(earlier.frame, screen.frame))
        {
            continue;
        }
        unique.push(screen.clone());
    }
    unique
}

#[cfg(target_os = "macos")]
fn log_screens(screens: &[ScreenInfo]) {
    for screen in screens {
        eprintln!(
            "tap: screen {} id {} scale {} origin ({:.0}, {:.0}) size {:.0}x{:.0}",
            screen.index,
            screen.screen_id,
            screen.scale,
            screen.frame.origin.x,
            screen.frame.origin.y,
            screen.frame.size.width,
            screen.frame.size.height
        );
    }
    for screen in screens {
        if let Some(earlier) = screens.iter().find(|earlier| {
            earlier.index < screen.index && rects_match(earlier.frame, screen.frame)
        }) {
            eprintln!(
                "tap: screen {} frame matches screen {}",
                screen.index, earlier.index
            );
        }
    }
}

/// Returns whether the window's frame matches `frame` after it is on screen.
#[cfg(target_os = "macos")]
fn present_macos_overlay(window: &tauri::WebviewWindow, idx: usize, frame: MacRect) -> bool {
    let Ok(ns_window) = window.ns_window() else {
        eprintln!("tap: overlay-{idx} has no ns window");
        return false;
    };
    let window_ptr = ns_window as *mut objc2::runtime::AnyObject;
    if window_ptr.is_null() {
        eprintln!("tap: overlay-{idx} has no ns window");
        return false;
    }
    unsafe {
        // CanJoinAllSpaces | Stationary | IgnoresCycle | FullScreenAuxiliary.
        // "Displays have separate Spaces" keeps a window on the space where it
        // was ordered in. These bits have to be set first, then the frame,
        // and only then may the window be ordered in.
        let behavior: usize = (1 << 0) | (1 << 4) | (1 << 6) | (1 << 8);
        let _: () = objc2::msg_send![window_ptr, setCollectionBehavior: behavior];
        let _: () = objc2::msg_send![window_ptr, setHidesOnDeactivate: false];
        // NSWindowAnimationBehaviorNone. NSInteger.
        let animation: isize = 2;
        let _: () = objc2::msg_send![window_ptr, setAnimationBehavior: animation];
        let _: () = objc2::msg_send![window_ptr, setMovable: false];
        let _: () = objc2::msg_send![window_ptr, setOpaque: false];
        let _: () = objc2::msg_send![window_ptr, setHasShadow: false];
        let _: () = objc2::msg_send![window_ptr, setIgnoresMouseEvents: true];
        let _: () = objc2::msg_send![window_ptr, setAlphaValue: 1.0f64];
        if let Some(color_cls) = objc2::runtime::AnyClass::get("NSColor") {
            let clear: *mut objc2::runtime::AnyObject = objc2::msg_send![color_cls, clearColor];
            if !clear.is_null() {
                let _: () = objc2::msg_send![window_ptr, setBackgroundColor: clear];
            }
        }
        // NSScreenSaverWindowLevel. NSInteger is 'q'; a 32-bit int aborts the process.
        let level: isize = 1000;
        let _: () = objc2::msg_send![window_ptr, setLevel: level];

        let _: () = objc2::msg_send![window_ptr, setFrame: frame display: true];
        if !rects_match(window_frame(window_ptr), frame) {
            // setFrame from the screen the window was created on (main, via
            // tao's center()) is often clamped back onto that screen. Park a
            // small rect on the target display first so AppKit adopts it,
            // then expand to the full frame. Still before order-in.
            let probe = probe_rect(frame);
            let _: () = objc2::msg_send![window_ptr, setFrameOrigin: probe.origin];
            let _: () = objc2::msg_send![window_ptr, setFrame: probe display: false];
            let _: () = objc2::msg_send![window_ptr, setFrame: frame display: true];
        }
        fit_webview(window_ptr);
        let actual = window_frame(window_ptr);
        if !rects_match(actual, frame) {
            eprintln!(
                "tap: overlay-{idx} frame after setFrame ({:.0}, {:.0}) {:.0}x{:.0} != requested ({:.0}, {:.0}) {:.0}x{:.0}",
                actual.origin.x,
                actual.origin.y,
                actual.size.width,
                actual.size.height,
                frame.origin.x,
                frame.origin.y,
                frame.size.width,
                frame.size.height
            );
        }
        // Not makeKeyAndOrderFront. Becoming key pulls the window onto the
        // active space and off the display we just targeted.
        let _: () = objc2::msg_send![window_ptr, orderFrontRegardless];
        let _: () = objc2::msg_send![window_ptr, setLevel: level];
        let _: () = objc2::msg_send![window_ptr, setIgnoresMouseEvents: true];
        let ordered = window_frame(window_ptr);
        if !rects_match(ordered, frame) && !rects_match(ordered, actual) {
            eprintln!(
                "tap: overlay-{idx} frame after orderFrontRegardless ({:.0}, {:.0}) {:.0}x{:.0} != requested ({:.0}, {:.0}) {:.0}x{:.0}",
                ordered.origin.x,
                ordered.origin.y,
                ordered.size.width,
                ordered.size.height,
                frame.origin.x,
                frame.origin.y,
                frame.size.width,
                frame.size.height
            );
        }
        rects_match(ordered, frame)
    }
}

#[cfg(target_os = "macos")]
fn window_frame(window_ptr: *mut objc2::runtime::AnyObject) -> MacRect {
    unsafe { objc2::msg_send![window_ptr, frame] }
}

#[cfg(target_os = "macos")]
fn probe_rect(frame: MacRect) -> MacRect {
    let width = 32.0_f64.min(frame.size.width);
    let height = 32.0_f64.min(frame.size.height);
    let x = frame.origin.x + (frame.size.width - width).min(8.0);
    let y = frame.origin.y + (frame.size.height - height).min(8.0);
    MacRect {
        origin: MacPoint { x, y },
        size: MacSize { width, height },
    }
}

/// The WKWebView is created while tao has the window centered on the main
/// display, often at a stale size. Stretch it to the content view before the
/// window is ordered in, and match the destination screen's scale.
#[cfg(target_os = "macos")]
fn fit_webview(window_ptr: *mut objc2::runtime::AnyObject) {
    unsafe {
        let content: *mut objc2::runtime::AnyObject = objc2::msg_send![window_ptr, contentView];
        if content.is_null() {
            return;
        }
        let mut bounds: MacRect = objc2::msg_send![content, bounds];
        let window_bounds: MacRect = objc2::msg_send![window_ptr, frame];
        if bounds.size.width < 1.0 || bounds.size.height < 1.0 {
            // Content view not laid out yet. A 0×0 webview paints no glow.
            bounds.origin = MacPoint { x: 0.0, y: 0.0 };
            bounds.size = window_bounds.size;
        }
        let scale: f64 = objc2::msg_send![window_ptr, backingScaleFactor];
        let subs: *mut objc2::runtime::AnyObject = objc2::msg_send![content, subviews];
        if subs.is_null() {
            return;
        }
        // Real NSArray (`__NSSingleObjectArrayI` and friends): `count` and
        // `objectAtIndex:` are NSUInteger ('Q'). The screen list is a Swift
        // array and uses NSInteger ('q') instead.
        let count: usize = objc2::msg_send![subs, count];
        let limit = count.min(8);
        for index in 0..limit {
            let sub: *mut objc2::runtime::AnyObject = objc2::msg_send![subs, objectAtIndex: index];
            if sub.is_null() {
                continue;
            }
            let _: () = objc2::msg_send![sub, setFrame: bounds];
            let _: () = objc2::msg_send![sub, setHidden: false];
            let layer: *mut objc2::runtime::AnyObject = objc2::msg_send![sub, layer];
            if !layer.is_null() {
                let _: () = objc2::msg_send![layer, setContentsScale: scale];
                let _: () = objc2::msg_send![layer, setNeedsDisplay];
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn ensure_screen_watch(app: &AppHandle) {
    use std::sync::atomic::Ordering;
    let _ = OVERLAY_APP.set(app.clone());
    static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    unsafe {
        let Some(superclass) = objc2::runtime::AnyClass::get("NSObject") else {
            return;
        };
        let Some(mut builder) = objc2::runtime::ClassBuilder::new("TapScreenWatch", superclass)
        else {
            return;
        };
        builder.add_method(
            objc2::sel!(screenChanged:),
            screen_changed as unsafe extern "C" fn(_, _, _),
        );
        let cls = builder.register();
        let obs: *mut objc2::runtime::AnyObject = objc2::msg_send![cls, alloc];
        let obs: *mut objc2::runtime::AnyObject = objc2::msg_send![obs, init];
        if obs.is_null() {
            return;
        }
        let Some(nc_cls) = objc2::runtime::AnyClass::get("NSNotificationCenter") else {
            return;
        };
        let center: *mut objc2::runtime::AnyObject = objc2::msg_send![nc_cls, defaultCenter];
        let Some(str_cls) = objc2::runtime::AnyClass::get("NSString") else {
            return;
        };
        let name: *mut objc2::runtime::AnyObject = objc2::msg_send![
            str_cls,
            stringWithUTF8String: c"NSApplicationDidChangeScreenParametersNotification".as_ptr()
        ];
        if center.is_null() || name.is_null() {
            return;
        }
        let _: () = objc2::msg_send![
            center,
            addObserver: obs,
            selector: objc2::sel!(screenChanged:),
            name: name,
            object: std::ptr::null_mut::<objc2::runtime::AnyObject>()
        ];
    }
}

#[cfg(target_os = "macos")]
unsafe extern "C" fn screen_changed(
    _this: &objc2::runtime::AnyObject,
    _cmd: objc2::runtime::Sel,
    _note: &objc2::runtime::AnyObject,
) {
    use std::sync::atomic::Ordering;
    if PLACING.load(Ordering::SeqCst) {
        return;
    }
    let Some(app) = OVERLAY_APP.get() else {
        return;
    };
    let Some(payload) = remembered_payload() else {
        return;
    };
    // A display was added, removed, or resized while the glow is up.
    // show_on_each_display no-ops when the frames are unchanged.
    let chrome = remembered_chrome();
    let _ = show_on_each_display(app, &payload, &chrome);
}
