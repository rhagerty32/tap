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

pub const PROTOCOL_VERSION: u8 = 2;
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
    /// Receiver is in do-not-disturb. Sent once in reply to Start.
    Busy,
    /// Current team. Members pass it on, so a join or a leave reaches everyone.
    Roster,
}

/// One person on the signed team list.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RosterMember {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub addrs: Vec<String>,
    /// This person left. Recipients remove them and pass the departure on.
    #[serde(default)]
    pub gone: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TapMessage {
    pub v: u8,
    pub kind: MessageKind,
    pub sender_id: String,
    pub sender_name: String,
    /// Kept so older packets still parse. v2 does not sign it; color is chosen by the receiver.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default)]
    pub status: String,
    #[serde(default, rename = "avatarHash")]
    pub avatar_hash: String,
    pub timestamp_ms: u64,
    pub nonce: String,
    /// Present only on roster messages. Empty for every other kind, so those
    /// signatures stay compatible with earlier builds.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<RosterMember>,
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
        MessageKind::Busy => "busy",
        MessageKind::Roster => "roster",
    }
}

pub fn canonical_member_list(members: &[RosterMember]) -> String {
    let mut members = members.to_vec();
    for member in &mut members {
        member.addrs.sort();
        member.addrs.dedup();
    }
    members.sort_by(|a, b| a.id.cmp(&b.id).then(a.gone.cmp(&b.gone)));
    serde_json::to_string(&members).unwrap_or_default()
}

pub fn canonical_payload(msg: &TapMessage) -> String {
    let base = format!(
        "v{}|{}|{}|{}|{}|{}|{}|{}",
        msg.v,
        kind_str(msg.kind),
        msg.sender_id,
        msg.sender_name,
        msg.status,
        msg.avatar_hash,
        msg.timestamp_ms,
        msg.nonce
    );
    if msg.members.is_empty() {
        base
    } else {
        format!("{base}|{}", canonical_member_list(&msg.members))
    }
}

pub fn mac_b64(key: &[u8], payload: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC key length");
    mac.update(payload.as_bytes());
    B64.encode(mac.finalize().into_bytes())
}

pub fn mac_matches(key: &[u8], payload: &str, sig_b64: &str) -> bool {
    let Ok(sig_bytes) = B64.decode(sig_b64) else {
        return false;
    };
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC key length");
    mac.update(payload.as_bytes());
    let expected = mac.finalize().into_bytes();
    sig_bytes.len() == expected.len() && bool::from(sig_bytes.as_slice().ct_eq(expected.as_slice()))
}

pub fn sign_message(key: &[u8], msg: &mut TapMessage) {
    msg.sig = Some(mac_b64(key, &canonical_payload(msg)));
}

pub fn verify_message(
    key: &[u8],
    msg: &TapMessage,
    replay: &mut ReplayGuard,
) -> Result<(), VerifyError> {
    if msg.v != PROTOCOL_VERSION {
        return Err(VerifyError::UnsupportedVersion);
    }
    let sig_b64 = msg.sig.as_ref().ok_or(VerifyError::MissingSignature)?;
    let sig_bytes = B64
        .decode(sig_b64)
        .map_err(|_| VerifyError::InvalidSignature)?;

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

pub fn new_outgoing(
    kind: MessageKind,
    sender_id: &str,
    sender_name: &str,
    status: &str,
    avatar_hash: &str,
) -> TapMessage {
    TapMessage {
        v: PROTOCOL_VERSION,
        kind,
        sender_id: sender_id.to_string(),
        sender_name: sender_name.to_string(),
        color: None,
        status: status.to_string(),
        avatar_hash: avatar_hash.to_string(),
        timestamp_ms: now_ms(),
        nonce: Uuid::new_v4().to_string(),
        members: Vec::new(),
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
        let mut msg = new_outgoing(MessageKind::Start, "alice", "Alice", "Headphones", "abc");
        sign_message(&key(), &mut msg);
        let mut guard = ReplayGuard::default();
        assert!(verify_message(&key(), &msg, &mut guard).is_ok());
    }

    #[test]
    fn rejects_bad_signature() {
        let mut msg = new_outgoing(MessageKind::Start, "alice", "Alice", "", "");
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
        let mut msg = new_outgoing(MessageKind::Heartbeat, "alice", "Alice", "", "");
        sign_message(&key(), &mut msg);
        let mut guard = ReplayGuard::default();
        assert!(verify_message(&key(), &msg, &mut guard).is_ok());
        assert!(matches!(
            verify_message(&key(), &msg, &mut guard),
            Err(VerifyError::Replay)
        ));
    }

    #[test]
    fn roster_signature_covers_who_joined_and_who_left() {
        let mut msg = new_outgoing(MessageKind::Roster, "ryan", "Ryan", "", "");
        msg.members = vec![RosterMember {
            id: "cole".into(),
            name: "Cole".into(),
            addrs: vec!["10.0.0.3:37561".into()],
            gone: false,
        }];
        sign_message(&key(), &mut msg);
        let mut guard = ReplayGuard::default();
        assert!(verify_message(&key(), &msg, &mut guard).is_ok());

        msg.members[0].gone = true;
        let mut guard = ReplayGuard::default();
        assert!(matches!(
            verify_message(&key(), &msg, &mut guard),
            Err(VerifyError::InvalidSignature)
        ));
    }

    #[test]
    fn rejects_stale_timestamp() {
        let mut msg = new_outgoing(MessageKind::Stop, "alice", "Alice", "", "");
        msg.timestamp_ms = now_ms().saturating_sub(60_000);
        sign_message(&key(), &mut msg);
        let mut guard = ReplayGuard::default();
        assert!(matches!(
            verify_message(&key(), &msg, &mut guard),
            Err(VerifyError::TimestampSkew)
        ));
    }
}
