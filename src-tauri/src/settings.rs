use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use uuid::Uuid;

pub const DEFAULT_PORT: u16 = 37561;
pub const DEFAULT_COLOR: &str = "#3b82f6";

fn default_port() -> u16 {
    DEFAULT_PORT
}

fn default_glow() -> String {
    "edge".into()
}

fn default_screens() -> String {
    "all".into()
}

fn default_banner() -> bool {
    true
}

fn default_theme() -> String {
    "system".into()
}

/// Colors assigned automatically. Kept stable so people without a saved color
/// do not change color when the chooser changes.
const AUTO_COLORS: [&str; 6] = [
    "#F7A53F", "#F86E51", "#E8638C", "#7C9A6E", "#6B8FB8", "#B47CC7",
];

/// Accept any `#RRGGBB` the hue and saturation dials can produce.
pub fn canonical_color(color: &str) -> Option<String> {
    let hex = color.trim().strip_prefix('#').unwrap_or(color.trim());
    if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    Some(format!("#{hex}").to_ascii_uppercase())
}

pub fn generate_team_key() -> String {
    Uuid::new_v4().simple().to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub display_name: String,
    pub sender_color: String,
    pub team_key: String,
    pub do_not_disturb: bool,
    pub chime_enabled: bool,
    pub launch_at_login: bool,
    pub tailnet_only: bool,
    pub manual_peers: Vec<String>,
    pub mute_senders: HashMap<String, bool>,
    pub allow_senders: HashMap<String, bool>,
    #[serde(default = "default_port")]
    pub listen_port: u16,
    /// When set, system Focus also pauses taps.
    #[serde(default)]
    pub follow_system_dnd: bool,
    /// User turned the popover switch off while system Focus was on.
    /// Cleared the next time Focus ends.
    #[serde(default)]
    pub suppress_system_dnd: bool,
    #[serde(default = "default_glow")]
    pub glow_style: String,
    #[serde(default = "default_banner")]
    pub show_banner: bool,
    /// `all`, `cursor`, or `selected`.
    #[serde(default = "default_screens")]
    pub overlay_screens: String,
    #[serde(default)]
    pub selected_displays: Vec<String>,
    /// Sender id → hex color, chosen on this machine.
    #[serde(default)]
    pub peer_colors: HashMap<String, String>,
    /// `system`, `light`, or `dark`.
    #[serde(default = "default_theme")]
    pub theme: String,
}

impl Default for Settings {
    fn default() -> Self {
        let hostname = hostname::get()
            .ok()
            .and_then(|h| h.into_string().ok())
            .unwrap_or_else(|| "Tap User".into());
        Self {
            display_name: hostname,
            sender_color: DEFAULT_COLOR.into(),
            team_key: String::new(),
            do_not_disturb: false,
            chime_enabled: false,
            launch_at_login: false,
            tailnet_only: false,
            manual_peers: Vec::new(),
            mute_senders: HashMap::new(),
            allow_senders: HashMap::new(),
            listen_port: DEFAULT_PORT,
            follow_system_dnd: false,
            suppress_system_dnd: false,
            glow_style: default_glow(),
            show_banner: true,
            overlay_screens: default_screens(),
            selected_displays: Vec::new(),
            peer_colors: HashMap::new(),
            theme: default_theme(),
        }
    }
}

pub fn effective_dnd(settings: &Settings, system_focus: bool) -> bool {
    if settings.do_not_disturb {
        return true;
    }
    settings.follow_system_dnd && system_focus && !settings.suppress_system_dnd
}

pub fn color_for_peer(settings: &Settings, sender_id: &str) -> String {
    if let Some(color) = settings.peer_colors.get(sender_id) {
        if let Some(canonical) = canonical_color(color) {
            return canonical.to_string();
        }
    }
    let mut hash: u32 = 0;
    for byte in sender_id.bytes() {
        hash = hash.wrapping_mul(31).wrapping_add(byte as u32);
    }
    AUTO_COLORS[(hash as usize) % AUTO_COLORS.len()].to_string()
}

pub fn normalize_glow(style: &str) -> String {
    match style {
        "soft" | "bold" => style.to_string(),
        _ => "edge".into(),
    }
}

pub fn normalize_screens(mode: &str) -> String {
    match mode {
        "cursor" | "selected" => mode.to_string(),
        _ => "all".into(),
    }
}

pub fn normalize_theme(theme: &str) -> String {
    match theme {
        "light" | "dark" => theme.to_string(),
        _ => "system".into(),
    }
}

pub fn settings_path() -> PathBuf {
    let base = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join("tap").join("settings.json")
}

pub fn load_settings() -> Settings {
    let path = settings_path();
    if let Ok(data) = fs::read_to_string(&path) {
        if let Ok(s) = serde_json::from_str(&data) {
            return s;
        }
    }
    Settings::default()
}

pub fn save_settings(settings: &Settings) -> Result<(), String> {
    let path = settings_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let data = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    fs::write(path, data).map_err(|e| e.to_string())
}

pub fn team_key_bytes(settings: &Settings) -> Option<Vec<u8>> {
    if settings.team_key.trim().is_empty() {
        None
    } else {
        Some(settings.team_key.as_bytes().to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::{canonical_color, color_for_peer, Settings, AUTO_COLORS};

    #[test]
    fn any_hex_is_saved_and_defaults_stay_in_the_original_six() {
        assert_eq!(canonical_color("#0f766e").as_deref(), Some("#0F766E"));
        assert_eq!(canonical_color("  #aabbcc ").as_deref(), Some("#AABBCC"));
        assert_eq!(canonical_color("#112"), None);
        assert_eq!(canonical_color("nope"), None);
        let mut settings = Settings::default();
        settings.peer_colors.insert("a".into(), "#112233".into());
        assert_eq!(color_for_peer(&settings, "a"), "#112233");
        let automatic = color_for_peer(&Settings::default(), "someone");
        assert!(AUTO_COLORS.contains(&automatic.as_str()));
    }
}

pub fn is_tailnet_ip(ip: &std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            let octets = v4.octets();
            octets[0] == 100 && (64..=127).contains(&octets[1])
        }
        _ => false,
    }
}
