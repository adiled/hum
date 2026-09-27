//! Peer liveness: probe, expire, evict.
//!
//! A peer is *live* while traffic has arrived from it within `ttl`. A
//! peer is *dead* once nothing has, and a dead peer is evicted from the
//! registry so the caller can redial it. `chi:"peer-ping"` is the
//! probe; any inbound tone at all renews the lease, so a busy peer
//! never needs pinging.
//!
//! The peer registry has no clock of its own — the daemon owns the
//! sweep, because the daemon owns the dialer that has something to do
//! with an eviction.

use std::time::Duration;

use crate::{Hid, Tone};

/// Wire-level chi for a liveness probe. Answered with `peer-pong`.
pub const PING_CHI: &str = "peer-ping";

/// Wire-level chi for a probe reply.
pub const PONG_CHI: &str = "peer-pong";

/// What the registry knows about a peer's reachability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Liveness {
    /// Traffic seen within `ttl`.
    Live,
    /// Nothing seen within `ttl`. Still installed, still routable, and
    /// expected to be reaped on the next sweep.
    Stale,
    /// The transport reported the link closed. Distinguished from
    /// `Stale` because it is a fact rather than an inference, so a
    /// redial is worth attempting immediately instead of after a sweep.
    Dead,
}

/// How a peer lease is renewed. The drainer stamps on every inbound
/// tone; a probe reply is just a tone that says "I'm still here" with
/// no payload behind it.
#[derive(Debug, Clone, PartialEq)]
pub enum LivenessSignal {
    /// Any inbound traffic — a probe reply is the cheapest kind.
    Traffic,
    /// The transport's receiver closed. Terminal for this connection.
    TransportClosed,
}

impl LivenessSignal {
    /// Whether this signal keeps a peer on the `Live` side of the
    /// lease. `TransportClosed` cannot: the link is gone regardless of
    /// how recently we heard from it.
    pub fn renews(self) -> bool {
        matches!(self, LivenessSignal::Traffic)
    }
}

/// One peer's lease.
#[derive(Debug, Clone)]
pub struct Lease {
    /// When this peer was installed. Silence counts from here, so a
    /// peer that never proves itself still expires: a black-hole link
    /// that swallows every probe would otherwise read as `Live` forever.
    pub since: std::time::Instant,
    /// Traffic last seen from this peer, `None` until it first answers.
    pub last_seen: Option<std::time::Instant>,
    /// Set when the transport reported the link closed.
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

    /// Apply an inbound signal.
    pub fn observe(&mut self, signal: LivenessSignal) {
        match signal {
            LivenessSignal::Traffic => self.last_seen = Some(std::time::Instant::now()),
            LivenessSignal::TransportClosed => self.closed = true,
        }
    }

    /// Classify against `ttl`. A closed link is `Dead` regardless of
    /// the clock — the transport told us, and no amount of recent
    /// traffic overturns that.
    pub fn state(&self, ttl: Duration) -> Liveness {
        if self.closed {
            return Liveness::Dead;
        }
        // An unproven peer is still on probation, not immortal: quiet
        // since install ages it out just like a peer that went quiet.
        let quiet = self.last_seen.unwrap_or(self.since);
        if quiet.elapsed() > ttl {
            Liveness::Stale
        } else {
            Liveness::Live
        }
    }

    /// Whether the next sweep should reap this peer.
    pub fn expired(&self, ttl: Duration) -> bool {
        self.state(ttl) != Liveness::Live
    }
}

/// A `peer-ping` carrying our own identity so the far end can key its
/// reply at us without a registry lookup.
pub fn ping_tone(from: &Hid, to: &Hid, seq: u64) -> Tone {
    serde_json::json!({
        "chi": PING_CHI,
        "rid": hum_identity::HumId::mint().to_string(),
        "from": from.to_hex(),
        "to": to.to_hex(),
        "seq": seq,
    })
}

/// A `peer-pong`, echoing the probe's seq so a caller can pair
/// request with reply.
pub fn pong_tone(from: &Hid, to: &Hid, seq: u64) -> Tone {
    serde_json::json!({
        "chi": PONG_CHI,
        "rid": hum_identity::HumId::mint().to_string(),
        "from": from.to_hex(),
        "to": to.to_hex(),
        "seq": seq,
    })
}

/// The seq a probe carries, or `None` if this isn't a probe.
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
        // Backdate rather than sleep.
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
        // The transport's word outranks the clock: a link it has
        // dropped is dead even if a tone had arrived a moment ago.
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
