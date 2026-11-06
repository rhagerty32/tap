// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Presence checks CoreAudio off to the side. A fault in that helper must
    // not start another menu-bar app.
    if std::env::args().any(|arg| arg == "--status-probe")
        || std::env::var_os("TAP_STATUS_PROBE").is_some()
    {
        #[cfg(target_os = "macos")]
        tap_lib::run_status_probe();
        #[cfg(not(target_os = "macos"))]
        {
            println!("{}", tap_lib::status::current_status());
            return;
        }
    }
    tap_lib::run()
}
