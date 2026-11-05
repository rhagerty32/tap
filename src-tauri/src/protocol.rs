use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::{HashSet, VecDeque};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use subtle::ConstantTimeEq;
use thiserror::Error;
use uuid::Uuid;

type HmacSha256 = Hmac<Sha256>;

pub const PROTOCOL_VERSION: u8 = 1;
pub const MAX_CLOCK_SKEW: Duration = Duration::from_secs(30);
pub const NONCE_TTL: Duration = Duration::from_secs(300);
pub const MAX_NONCE_CACHE: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    Start,
    Heartbeat,
    Stop,
    Presence,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TapMessage {
    pub v: u8,
    pub kind: MessageKind,
    pub sender_id: String,
    pub sender_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    pub timestamp_ms: u64,
    pub nonce: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sig: Option<String>,
}

#[derive(Debug, Error)]
pub enum VerifyError {
    #[error("unsupported protocol version")]
    UnsupportedVersion,
    #[error("missing signature")]
    MissingSignature,
    #[error("invalid signature")]
    InvalidSignature,
    #[error("timestamp out of range")]
    TimestampSkew,
    #[error("replay detected")]
    Replay,
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn kind_str(kind: MessageKind) -> &'static str {
    match kind {
        MessageKind::Start => "start",
        MessageKind::Heartbeat => "heartbeat",
        MessageKind::Stop => "stop",
        MessageKind::Presence => "presence",
    }
}

pub fn canonical_payload(msg: &TapMessage) -> String {
    let color = msg.color.as_deref().unwrap_or("");
    format!(
        "v{}|{}|{}|{}|{}|{}|{}",
        msg.v,
        kind_str(msg.kind),
        msg.sender_id,
        msg.sender_name,
        color,
        msg.timestamp_ms,
        msg.nonce
    )
}

pub fn sign_message(key: &[u8], msg: &mut TapMessage) {
    let payload = canonical_payload(msg);
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC key length");
    mac.update(payload.as_bytes());
    let result = mac.finalize().into_bytes();
    msg.sig = Some(B64.encode(result));
}

pub fn verify_message(key: &[u8], msg: &TapMessage, replay: &mut ReplayGuard) -> Result<(), VerifyError> {
    if msg.v != PROTOCOL_VERSION {
        return Err(VerifyError::UnsupportedVersion);
    }
    let sig_b64 = msg.sig.as_ref().ok_or(VerifyError::MissingSignature)?;
    let sig_bytes = B64.decode(sig_b64).map_err(|_| VerifyError::InvalidSignature)?;

    let payload = canonical_payload(msg);
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC key length");
    mac.update(payload.as_bytes());
    let expected = mac.finalize().into_bytes();
    if sig_bytes.len() != expected.len()
        || bool::from(sig_bytes.as_slice().ct_ne(expected.as_slice()))
    {
        return Err(VerifyError::InvalidSignature);
    }

    let now = now_ms();
    let skew = msg.timestamp_ms.abs_diff(now);
    if skew > MAX_CLOCK_SKEW.as_millis() as u64 {
        return Err(VerifyError::TimestampSkew);
    }

    if !replay.register(&msg.nonce, msg.timestamp_ms) {
        return Err(VerifyError::Replay);
    }

    Ok(())
}

#[derive(Debug, Default)]
pub struct ReplayGuard {
    nonces: HashSet<String>,
    order: VecDeque<(String, u64)>,
}

impl ReplayGuard {
    pub fn register(&mut self, nonce: &str, timestamp_ms: u64) -> bool {
        let now = now_ms();
        if now.saturating_sub(timestamp_ms) > NONCE_TTL.as_millis() as u64 {
            return false;
        }
        if self.nonces.contains(nonce) {
            return false;
        }
        self.nonces.insert(nonce.to_string());
        self.order.push_back((nonce.to_string(), timestamp_ms));
        while self.order.len() > MAX_NONCE_CACHE {
            if let Some((old, _)) = self.order.pop_front() {
                self.nonces.remove(&old);
            }
        }
        true
    }
}

pub fn new_outgoing(kind: MessageKind, sender_id: &str, sender_name: &str, color: Option<String>) -> TapMessage {
    TapMessage {
        v: PROTOCOL_VERSION,
        kind,
        sender_id: sender_id.to_string(),
        sender_name: sender_name.to_string(),
        color,
        timestamp_ms: now_ms(),
        nonce: Uuid::new_v4().to_string(),
        sig: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> Vec<u8> {
        b"test-team-key-32-bytes-min!!!!".to_vec()
    }

    #[test]
    fn sign_and_verify_roundtrip() {
        let mut msg = new_outgoing(MessageKind::Start, "alice", "Alice", Some("#ff5500".into()));
        sign_message(&key(), &mut msg);
        let mut guard = ReplayGuard::default();
        assert!(verify_message(&key(), &msg, &mut guard).is_ok());
    }

    #[test]
    fn rejects_bad_signature() {
        let mut msg = new_outgoing(MessageKind::Start, "alice", "Alice", Some("#ff5500".into()));
        sign_message(&key(), &mut msg);
        msg.sig = Some(B64.encode(b"tampered"));
        let mut guard = ReplayGuard::default();
        assert!(matches!(
            verify_message(&key(), &msg, &mut guard),
            Err(VerifyError::InvalidSignature)
        ));
    }

    #[test]
    fn rejects_replay() {
        let mut msg = new_outgoing(MessageKind::Heartbeat, "alice", "Alice", None);
        sign_message(&key(), &mut msg);
        let mut guard = ReplayGuard::default();
        assert!(verify_message(&key(), &msg, &mut guard).is_ok());
        assert!(matches!(
            verify_message(&key(), &msg, &mut guard),
            Err(VerifyError::Replay)
        ));
    }

    #[test]
    fn rejects_stale_timestamp() {
        let mut msg = new_outgoing(MessageKind::Stop, "alice", "Alice", None);
        msg.timestamp_ms = now_ms().saturating_sub(60_000);
        sign_message(&key(), &mut msg);
        let mut guard = ReplayGuard::default();
        assert!(matches!(
            verify_message(&key(), &msg, &mut guard),
            Err(VerifyError::TimestampSkew)
        ));
    }
}
