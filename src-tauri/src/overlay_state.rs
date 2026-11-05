use std::time::{Duration, Instant};

pub const HEARTBEAT_INTERVAL: Duration = Duration::from_millis(500);
pub const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(2);
/// Receiver safety cutoff. Heartbeats keep the glow up, but never past a minute.
pub const SAFETY_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveOverlay {
    pub sender_id: String,
    pub sender_name: String,
    pub color: String,
    pub started: Instant,
    pub last_heartbeat: Instant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverlaySnapshot {
    Hidden,
    Visible(ActiveOverlay),
}

#[derive(Debug, Default)]
pub struct OverlayStateMachine {
    active: Option<ActiveOverlay>,
}

impl OverlayStateMachine {
    pub fn apply_start(&mut self, sender_id: String, sender_name: String, color: String) {
        let now = Instant::now();
        self.active = Some(ActiveOverlay {
            sender_id,
            sender_name,
            color,
            started: now,
            last_heartbeat: now,
        });
    }

    pub fn apply_heartbeat(&mut self, sender_id: &str) {
        if let Some(active) = &mut self.active {
            if active.sender_id == sender_id {
                active.last_heartbeat = Instant::now();
            }
        }
    }

    pub fn apply_stop(&mut self, sender_id: &str) {
        if self
            .active
            .as_ref()
            .map(|a| a.sender_id.as_str() == sender_id)
            .unwrap_or(false)
        {
            self.active = None;
        }
    }

    pub fn tick(&mut self, now: Instant) {
        let clear = self.active.as_ref().is_some_and(|active| {
            now.saturating_duration_since(active.last_heartbeat) > HEARTBEAT_TIMEOUT
                || now.saturating_duration_since(active.started) >= SAFETY_TIMEOUT
        });
        if clear {
            self.active = None;
        }
    }

    pub fn snapshot(&self) -> OverlaySnapshot {
        match &self.active {
            Some(a) => OverlaySnapshot::Visible(a.clone()),
            None => OverlaySnapshot::Hidden,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clears_after_two_seconds_without_heartbeat() {
        let mut sm = OverlayStateMachine::default();
        sm.apply_start("bob".into(), "Bob".into(), "#00ffaa".into());
        assert!(matches!(sm.snapshot(), OverlaySnapshot::Visible(_)));

        let t0 = Instant::now();
        sm.tick(t0);
        assert!(matches!(sm.snapshot(), OverlaySnapshot::Visible(_)));

        sm.tick(t0 + Duration::from_millis(1500));
        assert!(matches!(sm.snapshot(), OverlaySnapshot::Visible(_)));

        sm.tick(t0 + Duration::from_millis(2001));
        assert!(matches!(sm.snapshot(), OverlaySnapshot::Hidden));
    }

    #[test]
    fn clears_after_sixty_seconds_even_with_fresh_heartbeats() {
        let mut sm = OverlayStateMachine::default();
        sm.apply_start("bob".into(), "Bob".into(), "#F86E51".into());
        if let Some(active) = &mut sm.active {
            active.started = Instant::now() - Duration::from_secs(61);
            active.last_heartbeat = Instant::now();
        }
        sm.tick(Instant::now());
        assert!(matches!(sm.snapshot(), OverlaySnapshot::Hidden));
    }

    #[test]
    fn heartbeat_extends_visibility() {
        let mut sm = OverlayStateMachine::default();
        sm.apply_start("bob".into(), "Bob".into(), "#00ffaa".into());
        std::thread::sleep(Duration::from_millis(1500));
        sm.apply_heartbeat("bob");
        std::thread::sleep(Duration::from_millis(1500));
        sm.tick(Instant::now());
        assert!(matches!(sm.snapshot(), OverlaySnapshot::Visible(_)));
    }
}
