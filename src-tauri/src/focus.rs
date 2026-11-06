//! Best-effort system Focus / do-not-disturb. The popover switch always works
//! without this. macOS only reports Focus to apps the user has allowed.

use std::sync::atomic::{AtomicBool, Ordering};

static CACHED: AtomicBool = AtomicBool::new(false);

pub fn cached_system_focus() -> bool {
    CACHED.load(Ordering::Relaxed)
}

pub fn refresh_system_focus() -> bool {
    let focused = query_system_focus();
    CACHED.store(focused, Ordering::Relaxed);
    focused
}

#[cfg(target_os = "macos")]
fn query_system_focus() -> bool {
    #[link(name = "Intents", kind = "framework")]
    extern "C" {}
    let Some(cls) = objc2::runtime::AnyClass::get("INFocusStatusCenter") else {
        return false;
    };
    unsafe {
        let center: *mut objc2::runtime::AnyObject = objc2::msg_send![cls, defaultCenter];
        if center.is_null() {
            return false;
        }
        let status: *mut objc2::runtime::AnyObject = objc2::msg_send![center, focusStatus];
        if status.is_null() {
            return false;
        }
        // `isFocused` is `NSNumber *`, not `BOOL`. A nil number means the app
        // is not allowed to read Focus.
        let number: *mut objc2::runtime::AnyObject = objc2::msg_send![status, isFocused];
        if number.is_null() {
            return false;
        }
        let focused: objc2::runtime::Bool = objc2::msg_send![number, boolValue];
        focused.as_bool()
    }
}

#[cfg(not(target_os = "macos"))]
fn query_system_focus() -> bool {
    let Ok(output) = std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.notifications", "show-banners"])
        .output()
    else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.trim() == "false"
}
