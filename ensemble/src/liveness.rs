use std::time::Duration;

use crate::{Hid, Tone};

pub const PING_CHI: &str = "peer-ping";

pub const PONG_CHI: &str = "peer-pong";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Liveness {
    Live,
    Stale,
    Dead,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LivenessSignal {
    Traffic,
    TransportClosed,
}

impl LivenessSignal {
    pub fn renews(self) -> bool {
        matches!(self, LivenessSignal::Traffic)
    }
}

#[derive(Debug, Clone)]
pub struct Lease {
    pub since: std::time::Instant,
    pub last_seen: Option<std::time::Instant>,
    pub closed: bool,
}

impl Default for Lease {
    fn default() -> Self {
        Self {
            since: std::time::Instant::now(),
            last_seen: None,
            closed: false,
        }
    }
}

impl Lease {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn observe(&mut self, signal: LivenessSignal) {
        match signal {
            LivenessSignal::Traffic => self.last_seen = Some(std::time::Instant::now()),
            LivenessSignal::TransportClosed => self.closed = true,
        }
    }

    pub fn state(&self, ttl: Duration) -> Liveness {
        if self.closed {
            return Liveness::Dead;
        }
        let quiet = self.last_seen.unwrap_or(self.since);
        if quiet.elapsed() > ttl {
            Liveness::Stale
        } else {
            Liveness::Live
        }
    }

    pub fn expired(&self, ttl: Duration) -> bool {
        self.state(ttl) != Liveness::Live
    }
}

pub fn ping_tone(from: &Hid, to: &Hid, seq: u64) -> Tone {
    serde_json::json!({
        "chi": PING_CHI,
        "rid": hum_identity::HumId::mint().to_string(),
        "from": from.to_hex(),
        "to": to.to_hex(),
        "seq": seq,
    })
}

pub fn pong_tone(from: &Hid, to: &Hid, seq: u64) -> Tone {
    serde_json::json!({
        "chi": PONG_CHI,
        "rid": hum_identity::HumId::mint().to_string(),
        "from": from.to_hex(),
        "to": to.to_hex(),
        "seq": seq,
    })
}

pub fn probe_seq(tone: &Tone) -> Option<u64> {
    if tone.get("chi").and_then(|v| v.as_str()) != Some(PING_CHI) {
        return None;
    }
    tone.get("seq").and_then(|v| v.as_u64())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_lease_is_live() {
        assert_eq!(Lease::new().state(Duration::from_secs(30)), Liveness::Live);
    }

    #[test]
    fn traffic_keeps_a_peer_live() {
        let mut l = Lease::new();
        l.observe(LivenessSignal::Traffic);
        assert_eq!(l.state(Duration::from_secs(30)), Liveness::Live);
        assert!(!l.expired(Duration::from_secs(30)));
    }

    #[test]
    fn quiet_past_ttl_goes_stale() {
        let mut l = Lease::new();
        l.observe(LivenessSignal::Traffic);
        l.last_seen = Some(std::time::Instant::now() - Duration::from_secs(31));
        assert_eq!(l.state(Duration::from_secs(30)), Liveness::Stale);
        assert!(l.expired(Duration::from_secs(30)));
    }

    #[test]
    fn closed_is_dead_even_when_fresh() {
        let mut l = Lease::new();
        l.observe(LivenessSignal::Traffic);
        l.observe(LivenessSignal::TransportClosed);
        assert_eq!(l.state(Duration::from_secs(30)), Liveness::Dead);
    }

    #[test]
    fn closed_survives_later_traffic() {
        let mut l = Lease::new();
        l.observe(LivenessSignal::TransportClosed);
        l.observe(LivenessSignal::Traffic);
        assert_eq!(l.state(Duration::from_secs(30)), Liveness::Dead);
        assert!(l.expired(Duration::from_secs(30)));
    }

    #[test]
    fn probe_seq_reads_only_pings() {
        let p = ping_tone(&Hid::random_humd(), &Hid::random_humd(), 7);
        assert_eq!(probe_seq(&p), Some(7));
        let c = pong_tone(&Hid::random_humd(), &Hid::random_humd(), 7);
        assert_eq!(probe_seq(&c), None);
    }

    #[test]
    fn ping_and_pong_carry_chi_and_seq() {
        let a = Hid::random_humd();
        let b = Hid::random_humd();
        assert_eq!(ping_tone(&a, &b, 3)["chi"], PING_CHI);
        assert_eq!(pong_tone(&a, &b, 3)["chi"], PONG_CHI);
        assert_eq!(ping_tone(&a, &b, 3)["seq"], 3);
    }

    #[test]
    fn transport_closed_does_not_renew() {
        assert!(!LivenessSignal::TransportClosed.renews());
        assert!(LivenessSignal::Traffic.renews());
    }
}
