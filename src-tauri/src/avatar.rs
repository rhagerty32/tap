//! Profile photo. A 64px JPEG lives next to settings. Presence carries a short
//! hash; peers pull the bytes over TCP on the same port as UDP when the hash changes.

use crate::protocol::{mac_b64, mac_matches, now_ms};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::time::Duration;

pub const MAX_AVATAR_BYTES: usize = 16 * 1024;

fn config_dir() -> PathBuf {
    crate::settings::settings_path()
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn avatar_file() -> PathBuf {
    config_dir().join("avatar.jpg")
}

fn cache_dir() -> PathBuf {
    config_dir().join("avatars")
}

pub fn cache_path(id: &str, hash: &str) -> PathBuf {
    let safe_id: String = id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    cache_dir().join(format!("{safe_id}-{hash}.jpg"))
}

pub fn hash_of(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())[..12].to_string()
}

pub fn current_hash() -> String {
    fs::read(avatar_file())
        .map(|bytes| hash_of(&bytes))
        .unwrap_or_default()
}

pub fn has_cache(id: &str, hash: &str) -> bool {
    cache_path(id, hash).is_file()
}

pub fn write_cache(id: &str, hash: &str, bytes: &[u8]) -> Result<(), String> {
    let path = cache_path(id, hash);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::write(path, bytes).map_err(|e| e.to_string())
}

pub fn data_url_for_sender(id: &str) -> Option<String> {
    let dir = cache_dir();
    let safe_id: String = id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    let prefix = format!("{safe_id}-");
    let mut match_path = None;
    for entry in fs::read_dir(dir).ok()? {
        let path = entry.ok()?.path();
        let name = path.file_name()?.to_string_lossy().to_string();
        if name.starts_with(&prefix) {
            match_path = Some(path);
        }
    }
    let bytes = fs::read(match_path?).ok()?;
    Some(format!(
        "data:image/jpeg;base64,{}",
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes)
    ))
}

pub fn data_url_for(id: &str, hash: &str) -> Option<String> {
    let bytes = fs::read(cache_path(id, hash)).ok()?;
    Some(format!(
        "data:image/jpeg;base64,{}",
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes)
    ))
}

pub fn own_data_url() -> Option<String> {
    let bytes = fs::read(avatar_file()).ok()?;
    Some(format!(
        "data:image/jpeg;base64,{}",
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes)
    ))
}

/// Shrink whatever the user picked into a small JPEG and store it.
pub fn store_avatar(bytes: &[u8]) -> Result<String, String> {
    let image = image::load_from_memory(bytes).map_err(|e| e.to_string())?;
    let thumb = image.resize(64, 64, image::imageops::FilterType::Triangle);
    let rgb = thumb.to_rgb8();
    let mut jpeg = Vec::new();
    for quality in [70u8, 45, 30] {
        jpeg.clear();
        let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, quality);
        encoder
            .encode(
                rgb.as_raw(),
                rgb.width(),
                rgb.height(),
                image::ExtendedColorType::Rgb8,
            )
            .map_err(|e| e.to_string())?;
        if jpeg.len() <= MAX_AVATAR_BYTES {
            break;
        }
    }
    if jpeg.len() > MAX_AVATAR_BYTES {
        return Err("Photo is still too large after shrinking.".into());
    }
    let path = avatar_file();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::write(&path, &jpeg).map_err(|e| e.to_string())?;
    Ok(hash_of(&jpeg))
}

pub fn handle_connection(mut stream: std::net::TcpStream, key: &[u8]) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    let mut buf = [0u8; 512];
    let mut filled = 0usize;
    while filled < buf.len() {
        match stream.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => {
                filled += n;
                if buf[..filled].contains(&b'\n') {
                    break;
                }
            }
            Err(_) => return,
        }
    }
    let line = String::from_utf8_lossy(&buf[..filled]);
    let mut parts = line.trim().split_whitespace();
    if parts.next() != Some("AVATAR") {
        let _ = stream.write_all(b"NO\n");
        return;
    }
    let Some(ts) = parts.next().and_then(|v| v.parse::<u64>().ok()) else {
        let _ = stream.write_all(b"NO\n");
        return;
    };
    let Some(nonce) = parts.next() else {
        let _ = stream.write_all(b"NO\n");
        return;
    };
    let Some(sig) = parts.next() else {
        let _ = stream.write_all(b"NO\n");
        return;
    };
    if now_ms().abs_diff(ts) > 30_000 {
        let _ = stream.write_all(b"NO\n");
        return;
    }
    let payload = format!("avatar|{ts}|{nonce}");
    if !mac_matches(key, &payload, sig) {
        let _ = stream.write_all(b"NO\n");
        return;
    }
    let Ok(bytes) = fs::read(avatar_file()) else {
        let _ = stream.write_all(b"NO\n");
        return;
    };
    if bytes.len() > MAX_AVATAR_BYTES {
        let _ = stream.write_all(b"NO\n");
        return;
    }
    let header = format!("OK {}\n", bytes.len());
    if stream.write_all(header.as_bytes()).is_err() || stream.write_all(&bytes).is_err() {
        return;
    }
}

pub fn fetch(addr: SocketAddr, key: &[u8]) -> Option<Vec<u8>> {
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .ok()?;
    let ts = now_ms();
    let nonce = uuid::Uuid::new_v4().to_string();
    let payload = format!("avatar|{ts}|{nonce}");
    let sig = mac_b64(key, &payload);
    stream
        .write_all(format!("AVATAR {ts} {nonce} {sig}\n").as_bytes())
        .ok()?;
    let mut header = Vec::new();
    let mut byte = [0u8; 1];
    while header.len() < 32 {
        let n = stream.read(&mut byte).ok()?;
        if n == 0 {
            return None;
        }
        if byte[0] == b'\n' {
            break;
        }
        header.push(byte[0]);
    }
    let header = String::from_utf8(header).ok()?;
    let mut parts = header.split_whitespace();
    if parts.next() != Some("OK") {
        return None;
    }
    let len: usize = parts.next()?.parse().ok()?;
    if len == 0 || len > MAX_AVATAR_BYTES {
        return None;
    }
    let mut body = vec![0u8; len];
    stream.read_exact(&mut body).ok()?;
    Some(body)
}
