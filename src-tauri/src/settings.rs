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

fn default_tailnet_only() -> bool {
    true
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
    #[serde(default = "default_tailnet_only")]
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
    /// People removed on this Mac. Discovery will not show them again.
    #[serde(default)]
    pub hidden_peers: Vec<String>,
    /// `system`, `light`, or `dark`.
    #[serde(default = "default_theme")]
    pub theme: String,
    /// Peer id → one letter or digit, used while the team list is open.
    #[serde(default)]
    pub peer_keys: HashMap<String, String>,
    /// Linux chord that opens the team list. Missing means "use the default".
    /// An empty string means the user turned it off.
    #[serde(default)]
    pub roster_shortcut: Option<String>,
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
            tailnet_only: default_tailnet_only(),
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
            hidden_peers: Vec::new(),
            theme: default_theme(),
            peer_keys: HashMap::new(),
            roster_shortcut: None,
        }
    }
}

pub const DEFAULT_ROSTER_SHORTCUT: &str = "ctrl+alt+shift+KeyT";

/// Chord that opens the team list. Empty clears it. A bare letter is rejected
/// so it cannot collide with the per-person keys.
pub fn normalize_roster_shortcut(raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(String::new());
    }
    let mut ctrl = false;
    let mut alt = false;
    let mut shift = false;
    let mut super_key = false;
    let mut key = None;
    for token in raw.split('+') {
        let token = token.trim();
        if token.is_empty() {
            return Err("Use a modifier, like Ctrl+Alt+Shift+T.".into());
        }
        match token.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => ctrl = true,
            "alt" | "option" | "opt" => alt = true,
            "shift" => shift = true,
            "super" | "meta" | "cmd" | "command" | "win" => super_key = true,
            _ => {
                if key.is_some() {
                    return Err("Use one key with the modifiers.".into());
                }
                key = Some(canonical_shortcut_key(token)?);
            }
        }
    }
    let Some(key) = key else {
        return Err("Use a modifier, like Ctrl+Alt+Shift+T.".into());
    };
    if !(ctrl || alt || shift || super_key) {
        return Err("Include Ctrl, Alt, Shift, or Super. A bare letter would fire while you type.".into());
    }
    let mut parts = Vec::new();
    if ctrl {
        parts.push("ctrl");
    }
    if alt {
        parts.push("alt");
    }
    if shift {
        parts.push("shift");
    }
    if super_key {
        parts.push("super");
    }
    parts.push(key.as_str());
    Ok(parts.join("+"))
}

/// GNOME custom-shortcut accelerator, such as `<Ctrl><Alt><Shift>t`.
pub fn gnome_accelerator(normalized: &str) -> String {
    let mut out = String::new();
    for token in normalized.split('+').filter(|token| !token.is_empty()) {
        match token {
            "ctrl" => out.push_str("<Ctrl>"),
            "alt" => out.push_str("<Alt>"),
            "shift" => out.push_str("<Shift>"),
            "super" => out.push_str("<Super>"),
            other => out.push_str(&gnome_key_name(other)),
        }
    }
    out
}

fn canonical_shortcut_key(token: &str) -> Result<String, String> {
    let compact = token.trim();
    if compact.len() == 1 {
        let ch = compact.chars().next().unwrap();
        if ch.is_ascii_alphabetic() {
            return Ok(format!("Key{}", ch.to_ascii_uppercase()));
        }
        if ch.is_ascii_digit() {
            return Ok(format!("Digit{ch}"));
        }
    }
    let upper = compact.to_ascii_uppercase();
    if let Some(rest) = upper.strip_prefix("KEY") {
        if rest.len() == 1 && rest.chars().all(|ch| ch.is_ascii_alphabetic()) {
            return Ok(format!("Key{rest}"));
        }
    }
    if let Some(rest) = upper.strip_prefix("DIGIT") {
        if rest.len() == 1 && rest.chars().all(|ch| ch.is_ascii_digit()) {
            return Ok(format!("Digit{rest}"));
        }
    }
    if upper.starts_with('F') && upper[1..].parse::<u8>().ok().is_some_and(|n| (1..=12).contains(&n)) {
        return Ok(upper);
    }
    if let Some(rest) = upper.strip_prefix("NUMPAD") {
        if rest.len() == 1 && rest.chars().all(|ch| ch.is_ascii_digit()) {
            return Ok(format!("Numpad{rest}"));
        }
    }
    const NAMED: &[(&str, &str)] = &[
        ("SPACE", "Space"),
        ("MINUS", "Minus"),
        ("EQUAL", "Equal"),
        ("COMMA", "Comma"),
        ("PERIOD", "Period"),
        ("SLASH", "Slash"),
        ("SEMICOLON", "Semicolon"),
        ("QUOTE", "Quote"),
        ("BRACKETLEFT", "BracketLeft"),
        ("BRACKETRIGHT", "BracketRight"),
        ("BACKSLASH", "Backslash"),
        ("BACKQUOTE", "Backquote"),
        ("TAB", "Tab"),
        ("ENTER", "Enter"),
        ("ESCAPE", "Escape"),
        ("ARROWUP", "ArrowUp"),
        ("ARROWDOWN", "ArrowDown"),
        ("ARROWLEFT", "ArrowLeft"),
        ("ARROWRIGHT", "ArrowRight"),
    ];
    for (name, canonical) in NAMED {
        if upper == *name {
            return Ok((*canonical).to_string());
        }
    }
    Err("That key can't open the team list.".into())
}

fn gnome_key_name(code: &str) -> String {
    if let Some(letter) = code.strip_prefix("Key") {
        return letter.to_ascii_lowercase();
    }
    if let Some(digit) = code.strip_prefix("Digit") {
        return digit.to_string();
    }
    if let Some(arrow) = code.strip_prefix("Arrow") {
        return arrow.to_ascii_lowercase();
    }
    if let Some(digit) = code.strip_prefix("Numpad") {
        return format!("KP_{digit}");
    }
    match code {
        "Space" => "space".into(),
        "Minus" => "minus".into(),
        "Equal" => "equal".into(),
        "Comma" => "comma".into(),
        "Period" => "period".into(),
        "Slash" => "slash".into(),
        "Semicolon" => "semicolon".into(),
        "Quote" => "apostrophe".into(),
        "BracketLeft" => "bracketleft".into(),
        "BracketRight" => "bracketright".into(),
        "Backslash" => "backslash".into(),
        "Backquote" => "grave".into(),
        "Tab" => "Tab".into(),
        "Enter" => "Return".into(),
        "Escape" => "Escape".into(),
        other => other.to_string(),
    }
}

/// One unmodified letter or digit, stored lowercase. An empty string clears the key.
pub fn normalize_hotkey(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    let mut chars = trimmed.chars();
    let Some(ch) = chars.next() else {
        return Ok(String::new());
    };
    if chars.next().is_some() || !ch.is_ascii_alphanumeric() {
        return Err("Use one letter or number.".into());
    }
    Ok(ch.to_ascii_lowercase().to_string())
}

/// `group` is every id for this person. A key already used by someone else is rejected.
/// `Err("taken:<peer id>")` names the other person.
pub fn assign_hotkey(
    settings: &mut Settings,
    peer_id: &str,
    raw: &str,
    group: &[&str],
) -> Result<(), String> {
    let key = normalize_hotkey(raw)?;
    let mut owners: Vec<&str> = group.iter().copied().filter(|id| !id.is_empty()).collect();
    if !owners.contains(&peer_id) {
        owners.push(peer_id);
    }
    if !key.is_empty() {
        if let Some((owner, _)) = settings.peer_keys.iter().find(|(id, existing)| {
            !owners.contains(&id.as_str()) && existing.eq_ignore_ascii_case(&key)
        }) {
            return Err(format!("taken:{owner}"));
        }
    }
    for id in &owners {
        settings.peer_keys.remove(*id);
    }
    if !key.is_empty() {
        settings.peer_keys.insert(peer_id.to_string(), key);
    }
    Ok(())
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
        "line" | "soft" | "wash" | "bold" | "corners" => style.to_string(),
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
    use super::{
        assign_hotkey, canonical_color, color_for_peer, gnome_accelerator, normalize_glow,
        normalize_hotkey, normalize_roster_shortcut, Settings, AUTO_COLORS, DEFAULT_ROSTER_SHORTCUT,
    };

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

    #[test]
    fn hotkeys_are_one_letter_and_unique() {
        assert_eq!(normalize_hotkey(" S ").as_deref(), Ok("s"));
        assert_eq!(normalize_hotkey("7").as_deref(), Ok("7"));
        assert_eq!(normalize_hotkey("  ").as_deref(), Ok(""));
        assert!(normalize_hotkey("ab").is_err());
        assert!(normalize_hotkey("!").is_err());

        let mut settings = Settings::default();
        assign_hotkey(&mut settings, "alex", "s", &["alex"]).unwrap();
        assert_eq!(settings.peer_keys.get("alex").map(String::as_str), Some("s"));
        let taken = assign_hotkey(&mut settings, "blair", "S", &["blair"]).unwrap_err();
        assert_eq!(taken, "taken:alex");
        assign_hotkey(&mut settings, "alex", "s", &["alex", "alex-host"]).unwrap();
        assert_eq!(settings.peer_keys.len(), 1);
        assign_hotkey(&mut settings, "alex", "", &["alex", "alex-host"]).unwrap();
        assert!(settings.peer_keys.is_empty());
    }

    #[test]
    fn roster_shortcut_needs_a_modifier_and_one_key() {
        assert_eq!(
            normalize_roster_shortcut(DEFAULT_ROSTER_SHORTCUT).as_deref(),
            Ok(DEFAULT_ROSTER_SHORTCUT)
        );
        assert_eq!(normalize_roster_shortcut("  ").as_deref(), Ok(""));
        assert_eq!(
            normalize_roster_shortcut("Shift+Ctrl+alt+t").as_deref(),
            Ok("ctrl+alt+shift+KeyT")
        );
        assert_eq!(
            normalize_roster_shortcut("super+Digit1").as_deref(),
            Ok("super+Digit1")
        );
        assert!(normalize_roster_shortcut("t").is_err());
        assert!(normalize_roster_shortcut("ctrl+t+a").is_err());
        assert_eq!(
            gnome_accelerator("ctrl+alt+shift+KeyT"),
            "<Ctrl><Alt><Shift>t"
        );
        assert_eq!(gnome_accelerator(""), "");
    }

    #[test]
    fn glow_styles_keep_known_names() {
        for style in ["edge", "line", "soft", "wash", "bold", "corners"] {
            assert_eq!(normalize_glow(style), style);
        }
        assert_eq!(normalize_glow("neon"), "edge");
        assert_eq!(normalize_glow(""), "edge");
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
