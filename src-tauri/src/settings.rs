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
        }
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

pub fn is_tailnet_ip(ip: &std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            let octets = v4.octets();
            octets[0] == 100 && (64..=127).contains(&octets[1])
        }
        _ => false,
    }
}
