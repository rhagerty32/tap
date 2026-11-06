//! Automatic presence line. First match wins.

pub const IDLE_SECS: f64 = 180.0;

pub fn pick_status(camera: bool, mic: bool, headphones: bool, idle: bool) -> &'static str {
    if camera {
        "In a meeting"
    } else if mic {
        "On a call"
    } else if headphones {
        "Headphones"
    } else if idle {
        "Away"
    } else {
        "Online"
    }
}

pub fn current_status() -> String {
    pick_status(
        camera_in_use(),
        mic_in_use(),
        headphones_output(),
        idle_long(),
    )
    .to_string()
}

#[cfg(target_os = "macos")]
fn fourcc(bytes: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*bytes)
}

#[cfg(target_os = "macos")]
#[repr(C)]
struct AudioAddress {
    selector: u32,
    scope: u32,
    element: u32,
}

#[cfg(target_os = "macos")]
fn audio_u32(id: u32, selector: u32, scope: u32) -> Option<u32> {
    #[link(name = "CoreAudio", kind = "framework")]
    extern "C" {
        fn AudioObjectGetPropertyData(
            id: u32,
            address: *const AudioAddress,
            qualifier_size: u32,
            qualifier: *const std::ffi::c_void,
            size: *mut u32,
            data: *mut std::ffi::c_void,
        ) -> i32;
    }
    let address = AudioAddress {
        selector,
        scope,
        element: 0,
    };
    let mut size = 4u32;
    let mut value = 0u32;
    let status = unsafe {
        AudioObjectGetPropertyData(
            id,
            &address,
            0,
            std::ptr::null(),
            &mut size,
            &mut value as *mut u32 as *mut _,
        )
    };
    if status == 0 {
        Some(value)
    } else {
        None
    }
}

#[cfg(target_os = "macos")]
fn audio_string(id: u32, selector: u32, scope: u32, qualifier: Option<u32>) -> Option<String> {
    #[link(name = "CoreAudio", kind = "framework")]
    extern "C" {
        fn AudioObjectGetPropertyData(
            id: u32,
            address: *const AudioAddress,
            qualifier_size: u32,
            qualifier: *const std::ffi::c_void,
            size: *mut u32,
            data: *mut std::ffi::c_void,
        ) -> i32;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFStringGetCString(
            s: *const std::ffi::c_void,
            buf: *mut i8,
            size: isize,
            encoding: u32,
        ) -> u8;
        fn CFRelease(cf: *const std::ffi::c_void);
    }
    let address = AudioAddress {
        selector,
        scope,
        element: 0,
    };
    let mut cf: *const std::ffi::c_void = std::ptr::null();
    let mut size = std::mem::size_of::<*const std::ffi::c_void>() as u32;
    let mut qualifier_value = qualifier.unwrap_or(0);
    let (qsize, qptr): (u32, *const std::ffi::c_void) = if qualifier.is_some() {
        (
            4,
            &mut qualifier_value as *mut u32 as *const std::ffi::c_void,
        )
    } else {
        (0, std::ptr::null())
    };
    let status = unsafe {
        AudioObjectGetPropertyData(
            id,
            &address,
            qsize,
            qptr,
            &mut size,
            &mut cf as *mut _ as *mut _,
        )
    };
    if status != 0 || cf.is_null() {
        return None;
    }
    let mut buf = [0u8; 512];
    let ok = unsafe {
        CFStringGetCString(
            cf,
            buf.as_mut_ptr() as *mut i8,
            buf.len() as isize,
            0x0800_0100,
        )
    };
    unsafe { CFRelease(cf) };
    if ok == 0 {
        return None;
    }
    let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
    Some(String::from_utf8_lossy(&buf[..end]).into_owned())
}

#[cfg(target_os = "macos")]
fn default_device(output: bool) -> Option<u32> {
    let selector = if output {
        fourcc(b"dOut")
    } else {
        fourcc(b"dIn ")
    };
    let id = audio_u32(1, selector, fourcc(b"glob"))?;
    if id == 0 {
        None
    } else {
        Some(id)
    }
}

#[cfg(target_os = "macos")]
fn device_running(id: u32) -> bool {
    audio_u32(id, fourcc(b"gone"), fourcc(b"glob")).unwrap_or(0) != 0
}

#[cfg(target_os = "macos")]
fn headphones_output() -> bool {
    let Some(device) = default_device(true) else {
        return false;
    };
    let transport = audio_u32(device, fourcc(b"tran"), fourcc(b"glob")).unwrap_or(0);
    let bluetooth = fourcc(b"blue");
    let ble = fourcc(b"blea");
    if transport == bluetooth || transport == ble {
        return true;
    }
    let name = audio_string(device, fourcc(b"lnam"), fourcc(b"glob"), None).unwrap_or_default();
    let source = audio_u32(device, fourcc(b"ssrc"), fourcc(b"outp"));
    let source_name = source
        .and_then(|id| audio_string(device, fourcc(b"lscn"), fourcc(b"outp"), Some(id)))
        .unwrap_or_default();
    let label = format!("{name} {source_name}").to_lowercase();
    if looks_like_headphones(&label) {
        return true;
    }
    // Built-in jack uses a data source on the same device as the speakers.
    matches!(source, Some(id) if id == fourcc(b"hdpn") || id == fourcc(b"hdph"))
}

#[cfg(target_os = "macos")]
fn mic_in_use() -> bool {
    default_device(false).is_some_and(device_running)
}

#[cfg(target_os = "macos")]
fn camera_in_use() -> bool {
    #[link(name = "CoreMediaIO", kind = "framework")]
    extern "C" {
        fn CMIOObjectGetPropertyDataSize(
            id: u32,
            address: *const AudioAddress,
            qualifier_size: u32,
            qualifier: *const std::ffi::c_void,
            size: *mut u32,
        ) -> i32;
        fn CMIOObjectGetPropertyData(
            id: u32,
            address: *const AudioAddress,
            qualifier_size: u32,
            qualifier: *const std::ffi::c_void,
            data_size: u32,
            data_used: *mut u32,
            data: *mut std::ffi::c_void,
        ) -> i32;
    }
    let list_addr = AudioAddress {
        selector: fourcc(b"dev#"),
        scope: fourcc(b"glob"),
        element: 0,
    };
    let mut size = 0u32;
    let status =
        unsafe { CMIOObjectGetPropertyDataSize(1, &list_addr, 0, std::ptr::null(), &mut size) };
    if status != 0 || size == 0 || size > 4096 {
        return false;
    }
    let count = (size as usize) / 4;
    let mut ids = vec![0u32; count];
    // Unlike AudioObjectGetPropertyData, the buffer size is passed by value
    // and bytes-used comes back through a separate pointer.
    let mut used = 0u32;
    let status = unsafe {
        CMIOObjectGetPropertyData(
            1,
            &list_addr,
            0,
            std::ptr::null(),
            size,
            &mut used,
            ids.as_mut_ptr() as *mut _,
        )
    };
    if status != 0 {
        return false;
    }
    let count = (used as usize / 4).min(ids.len());
    let running = AudioAddress {
        selector: fourcc(b"gone"),
        scope: fourcc(b"glob"),
        element: 0,
    };
    for id in ids.into_iter().take(count) {
        let mut flag = 0u32;
        let mut flag_used = 0u32;
        let status = unsafe {
            CMIOObjectGetPropertyData(
                id,
                &running,
                0,
                std::ptr::null(),
                4,
                &mut flag_used,
                &mut flag as *mut u32 as *mut _,
            )
        };
        if status == 0 && flag != 0 {
            return true;
        }
    }
    false
}

#[cfg(target_os = "macos")]
fn idle_long() -> bool {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventSourceSecondsSinceLastEventType(state: u32, event_type: u32) -> f64;
    }
    let seconds = unsafe { CGEventSourceSecondsSinceLastEventType(0, u32::MAX) };
    seconds.is_finite() && seconds >= IDLE_SECS
}

#[cfg(not(target_os = "macos"))]
fn headphones_output() -> bool {
    sink_text().is_some_and(|text| {
        let lower = text.to_lowercase();
        lower.contains("headphone")
            || lower.contains("headset")
            || lower.contains("bluetooth")
            || lower.contains("form_factor = \"headset\"")
            || lower.contains("form_factor = \"headphones\"")
    })
}

#[cfg(not(target_os = "macos"))]
fn mic_in_use() -> bool {
    source_text().is_some_and(|text| {
        let lower = text.to_lowercase();
        lower.contains("state = \"running\"") || lower.contains("state: running")
    })
}

#[cfg(not(target_os = "macos"))]
fn camera_in_use() -> bool {
    let Ok(output) = std::process::Command::new("wpctl").arg("status").output() else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut in_video = false;
    for line in text.lines() {
        if line.starts_with("Video") {
            in_video = true;
            continue;
        }
        if in_video && !line.starts_with(' ') && !line.starts_with('\t') && !line.is_empty() {
            break;
        }
        if in_video {
            let lower = line.to_lowercase();
            if lower.contains("running") && (lower.contains("camera") || lower.contains("video")) {
                return true;
            }
        }
    }
    false
}

#[cfg(not(target_os = "macos"))]
fn idle_long() -> bool {
    let Ok(output) = std::process::Command::new("dbus-send")
        .args([
            "--session",
            "--print-reply",
            "--dest=org.freedesktop.ScreenSaver",
            "/org/freedesktop/ScreenSaver",
            "org.freedesktop.ScreenSaver.GetSessionIdleTime",
        ])
        .output()
    else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let seconds = text
        .split_whitespace()
        .last()
        .and_then(|n| n.parse::<f64>().ok());
    seconds.is_some_and(|s| s >= IDLE_SECS)
}

#[cfg(not(target_os = "macos"))]
fn sink_text() -> Option<String> {
    command_text("wpctl", &["inspect", "@DEFAULT_AUDIO_SINK@"])
        .or_else(|| command_text("pactl", &["get-default-sink"]))
}

#[cfg(not(target_os = "macos"))]
fn source_text() -> Option<String> {
    command_text("wpctl", &["inspect", "@DEFAULT_AUDIO_SOURCE@"])
}

#[cfg(not(target_os = "macos"))]
fn command_text(bin: &str, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new(bin).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(target_os = "macos")]
fn looks_like_headphones(label: &str) -> bool {
    [
        "headphone",
        "headset",
        "airpod",
        "earpod",
        "earbuds",
        "buds",
    ]
    .iter()
    .any(|needle| label.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::pick_status;

    #[test]
    fn camera_beats_headphones() {
        assert_eq!(pick_status(true, true, true, true), "In a meeting");
    }

    #[test]
    fn mic_beats_headphones() {
        assert_eq!(pick_status(false, true, true, false), "On a call");
    }

    #[test]
    fn headphones_before_idle() {
        assert_eq!(pick_status(false, false, true, true), "Headphones");
    }

    #[test]
    fn idle_then_online() {
        assert_eq!(pick_status(false, false, false, true), "Away");
        assert_eq!(pick_status(false, false, false, false), "Online");
    }
}
