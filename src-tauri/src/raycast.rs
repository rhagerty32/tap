//! Installed Raycast commands, so a trackpad corner can open one.
//!
//! Store and local extensions keep a `package.json` manifest. Built-in Raycast
//! commands (window management, clipboard, and the rest) are not in that
//! folder; those are assigned by pasting the deeplink Raycast copies.

use serde::Serialize;
use serde_json::Value;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RaycastCommand {
    pub title: String,
    pub extension: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RaycastCatalog {
    pub installed: bool,
    pub commands: Vec<RaycastCommand>,
}

#[tauri::command]
pub fn raycast_catalog() -> RaycastCatalog {
    #[cfg(target_os = "macos")]
    {
        catalog()
    }
    #[cfg(not(target_os = "macos"))]
    {
        RaycastCatalog {
            installed: false,
            commands: Vec::new(),
        }
    }
}

#[cfg(target_os = "macos")]
fn catalog() -> RaycastCatalog {
    let mut commands = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        let home = Path::new(&home);
        collect_extensions(
            &mut commands,
            &home.join("Library/Application Support/com.raycast.macos/extensions"),
        );
        collect_extensions(&mut commands, &home.join(".config/raycast/extensions"));
    }
    commands.sort_by(|a, b| {
        a.extension
            .to_lowercase()
            .cmp(&b.extension.to_lowercase())
            .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
            .then_with(|| a.url.cmp(&b.url))
    });
    commands.dedup_by(|a, b| a.url == b.url);
    RaycastCatalog {
        installed: raycast_installed(),
        commands,
    }
}

#[cfg(target_os = "macos")]
fn raycast_installed() -> bool {
    if Path::new("/Applications/Raycast.app").is_dir() {
        return true;
    }
    std::env::var_os("HOME")
        .map(|home| Path::new(&home).join("Applications/Raycast.app").is_dir())
        .unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn collect_extensions(out: &mut Vec<RaycastCommand>, root: &Path) {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let manifest = entry.path().join("package.json");
        let Ok(text) = fs::read_to_string(&manifest) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        out.extend(commands_from_manifest(&value));
    }
}

pub fn commands_from_manifest(value: &Value) -> Vec<RaycastCommand> {
    let author = text(value.get("owner"))
        .filter(|owner| !owner.is_empty())
        .or_else(|| text(value.get("author")))
        .unwrap_or_default();
    let name = text(value.get("name")).unwrap_or_default();
    let extension = text(value.get("title")).unwrap_or_else(|| name.clone());
    if author.is_empty() || name.is_empty() {
        return Vec::new();
    }
    let Some(commands) = value.get("commands").and_then(Value::as_array) else {
        return Vec::new();
    };
    commands
        .iter()
        .filter_map(|command| {
            if text(command.get("mode")).as_deref() == Some("menu-bar") {
                return None;
            }
            let command_name = text(command.get("name"))?;
            if command_name.is_empty() {
                return None;
            }
            let title = text(command.get("title")).unwrap_or_else(|| command_name.clone());
            Some(RaycastCommand {
                title,
                extension: extension.clone(),
                url: format!(
                    "raycast://extensions/{}/{}/{}",
                    path_segment(&author),
                    path_segment(&name),
                    path_segment(&command_name)
                ),
            })
        })
        .collect()
}

fn text(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(str::trim).map(str::to_string)
}

fn path_segment(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_deeplinks_and_skips_menu_bar_commands() {
        let manifest = serde_json::json!({
            "name": "color-picker",
            "title": "Color Picker",
            "author": "thomas",
            "commands": [
                { "name": "pick-color", "title": "Pick Color", "mode": "no-view" },
                { "name": "menu-bar", "title": "Menu Bar", "mode": "menu-bar" }
            ]
        });
        let commands = commands_from_manifest(&manifest);
        assert_eq!(
            commands,
            vec![RaycastCommand {
                title: "Pick Color".into(),
                extension: "Color Picker".into(),
                url: "raycast://extensions/thomas/color-picker/pick-color".into(),
            }]
        );
    }

    #[test]
    fn owner_wins_over_author() {
        let manifest = serde_json::json!({
            "name": "github",
            "title": "GitHub",
            "author": "thomas",
            "owner": "raycast",
            "commands": [{ "name": "my-issues", "title": "My Issues" }]
        });
        let commands = commands_from_manifest(&manifest);
        assert_eq!(
            commands[0].url,
            "raycast://extensions/raycast/github/my-issues"
        );
    }
}
