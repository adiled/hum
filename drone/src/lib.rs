//! Drone — hum's sentinel, the awareness wired across the thrum.
//!
//! Nobody invokes the drone. It observes the tones flowing through and
//! the events surfaced by the nestler, keeps a [`DroneState`] per
//! `sigil`, and on demand returns an [`Assessment`] describing what it
//! thinks should happen next. The host owns the heartbeat timer and the
//! retry plumbing; this crate is pure state.
//!
//! Drone is **LLM-agnostic**. It knows nothing about Claude, GPT, or
//! any specific model — context-loss pattern detection plugs in via
//! the [`Classifier`] trait. The default [`NoopClassifier`] never
//! flags anything; concrete classifiers live in nest-side crates
//! (e.g. `nest-common::RegexClassifier`).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thrum_core::{Chi, Tone};

/// How loud a classifier is shouting about a piece of LLM output.
///
/// - `None`     — text looks fine
/// - `Soft`     — flagged for evaluator-driven adjudication
/// - `Heavy`    — strongly flagged; evaluator may still confirm
/// - `Critical` — bypass the evaluator and swallow immediately
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Suspicion {
    None,
    Soft,
    Heavy,
    Critical,
}

impl Suspicion {
    /// True when any tier matched.
    pub fn flagged(self) -> bool {
        !matches!(self, Suspicion::None)
    }
}

/// Context-loss heuristic seam.
///
/// The drone calls this on `TurnEnd` (and during `assess`) to score
/// the accumulated response text. Implementations decide which
/// patterns are which severity; the drone only branches on the
/// returned [`Suspicion`].
///
/// Default impl is [`NoopClassifier`] (always [`Suspicion::None`]).
/// Concrete pattern-bank impls live outside this crate — see
/// `nest-common` for the regex-driven one tuned for chat-LLM context loss.
pub trait Classifier: Send + Sync {
    fn classify(&self, text: &str) -> Suspicion;
}

/// No-op default — every input is `Suspicion::None`. Drone running with
/// this classifier behaves as a pure channel-health sentinel; it
/// never reaches the swallow path on its own.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopClassifier;

impl Classifier for NoopClassifier {
    fn classify(&self, _text: &str) -> Suspicion {
        Suspicion::None
    }
}

/// What the drone thinks the host should do.
///
/// `unified` is the one-word verdict the host actually steers on;
/// `raw` carries the full diagnostic — populated even when unified is `Ok`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Assessment {
    pub unified: Verdict,
    pub raw: RawAssessment,
}

/// Coarse verdict — the only thing the dispatch loop should branch on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Verdict {
    /// nominal
    Ok,
    /// wane drift / desync — host should resync state
    Drift,
    /// missed beats past threshold — host should tear the channel down
    Dead,
    /// suspicious response detected — host should retry the last turn
    Swallow,
    /// echo timed out — host should re-send the tracked tone
    Retry,
}

/// Detailed read of the state at the moment of [`Drone::assess`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawAssessment {
    pub sigil: String,
    pub health: Health,
    pub suspicion: Suspicion,
    pub missed_beats: u32,
    pub inflight_tools: u32,
    pub pending_permissions: u32,
    pub pending_echoes: u32,
    pub local_wane: u64,
    pub remote_wane: u64,
    /// Why the unified verdict came out the way it did, free-form.
    pub reason: String,
}

/// Pre-verdict mood — the TS `Assessment` strings, now isolated to the
/// `raw` channel so they don't compete with [`Verdict`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Health {
    Serene,
    Alert,
    Tense,
    Critical,
}

impl Health {
    /// Beat interval (ms) that pairs with this mood. The host uses it
    /// to schedule the next [`DroneBeat`].
    pub fn rhythm_ms(self) -> u64 {
        match self {
            Health::Serene => 30_000,
            Health::Alert => 5_000,
            Health::Tense => 1_000,
            Health::Critical => 500,
        }
    }
}

/// Per-sigil ledger. Mutated by [`Drone::heard`] / [`Drone::observed`],
/// read by [`Drone::assess`]. Public for inspection; construct via
/// [`create_drone_state`] or let the [`Drone`] manage it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DroneState {
    pub sigil: String,
    pub health: Health,
    pub rhythm_ms: u64,
    pub local_wane: u64,
    pub remote_wane: u64,
    pub pending_echoes: HashMap<String, PendingEcho>,
    pub last_beat_sent: i64,
    pub last_beat_received: i64,
    pub missed_beats: u32,
    pub inflight_tools: u32,
    pub pending_permissions: u32,
    pub tokens_burned: u64,
    pub response_text: String,
    pub suspicious: Suspicion,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingEcho {
    pub rid: String,
    pub chi: String,
    pub time_ms: i64,
    pub retries: u32,
}

pub fn create_drone_state(sigil: impl Into<String>) -> DroneState {
    DroneState {
        sigil: sigil.into(),
        health: Health::Serene,
        rhythm_ms: Health::Serene.rhythm_ms(),
        local_wane: 0,
        remote_wane: 0,
        pending_echoes: HashMap::new(),
        last_beat_sent: 0,
        last_beat_received: 0,
        missed_beats: 0,
        inflight_tools: 0,
        pending_permissions: 0,
        tokens_burned: 0,
        response_text: String::new(),
        suspicious: Suspicion::None,
    }
}

/// What the nestler tells us about the LLM stream.
#[derive(Debug, Clone)]
pub enum Observed {
    ToolStart { name: Option<String> },
    ToolEnd { name: Option<String> },
    Tokens { delta: u64 },
    PermissionAsk,
    PermissionResolved,
    TextDelta { text: String },
    /// Turn boundary — triggers suspicion classification on response_text.
    TurnEnd,
}

/// Periodic heartbeat payload. Hosts serialize this into a `chi:"drone"`
/// tone body and emit on the rhythm tick.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DroneBeat {
    pub sigil: String,
    pub wane: u64,
    pub health: Health,
    pub rhythm_ms: u64,
    pub pending_echoes: Vec<String>,
    pub load: BeatLoad,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BeatLoad {
    pub active_sigils: u32,
    pub pending_permissions: u32,
    pub inflight_tools: u32,
    pub tokens_burned: u64,
}

/// Plug-in seat for an LLM-driven judge. v0 ships only the heuristic;
/// a future crate can implement this trait to refine `swallow`.
pub trait Evaluator: Send + Sync {
    /// Probability (0.0..=1.0) that `text` represents real context loss
    /// given `state`. The host calls this only when the heuristic
    /// flagged a suspicion — it is not on the hot path.
    fn evaluate(&self, text: &str, state: &DroneState) -> f32;
}

/// chis we track for echo correlation. Mirrors `Drone.TRACKED_CHI` in TS.
fn tracked(chi: Chi) -> bool {
    matches!(chi, Chi::Prompt | Chi::Cancel | Chi::ReleasePermit)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// The sentinel. Cheap to clone — internal state lives behind an `Arc`.
#[derive(Clone)]
pub struct Drone {
    inner: Arc<Inner>,
}

struct Inner {
    states: RwLock<HashMap<String, DroneState>>,
    classifier: Arc<dyn Classifier>,
    evaluator: Option<Arc<dyn Evaluator>>,
    swallow_threshold: f32,
}

impl Default for Drone {
    fn default() -> Self {
        Self::new()
    }
}

impl Drone {
    /// Pure channel-health drone. No context-loss detection — the
    /// classifier is a no-op. Use this when the host has no opinion
    /// about LLM output patterns and only wants the wane/echo/beat
    /// machinery.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                states: RwLock::new(HashMap::new()),
                classifier: Arc::new(NoopClassifier),
                evaluator: None,
                swallow_threshold: 0.7,
            }),
        }
    }

    /// Drone with a pluggable classifier. Pass a regex-bank impl
    /// (e.g. `nest_common::RegexClassifier`) to enable the swallow
    /// path on context-loss patterns.
    pub fn with_classifier(classifier: Arc<dyn Classifier>) -> Self {
        Self {
            inner: Arc::new(Inner {
                states: RwLock::new(HashMap::new()),
                classifier,
                evaluator: None,
                swallow_threshold: 0.7,
            }),
        }
    }

    /// Drone with both a classifier and an LLM judge. The classifier
    /// is the cheap regex first-gate; the evaluator is the optional
    /// adjudicator the drone consults when the classifier flags
    /// `Soft` or `Heavy`.
    pub fn with_classifier_and_evaluator(
        classifier: Arc<dyn Classifier>,
        evaluator: Arc<dyn Evaluator>,
        swallow_threshold: f32,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                states: RwLock::new(HashMap::new()),
                classifier,
                evaluator: Some(evaluator),
                swallow_threshold,
            }),
        }
    }

    /// Back-compat: drone with only an LLM judge, classifier = noop.
    /// Equivalent to `with_classifier_and_evaluator(NoopClassifier, …)`
    /// but `NoopClassifier::classify` always returns `None`, so the
    /// evaluator is unreachable in practice. Prefer
    /// `with_classifier_and_evaluator`.
    pub fn with_evaluator(evaluator: Arc<dyn Evaluator>, swallow_threshold: f32) -> Self {
        Self::with_classifier_and_evaluator(Arc::new(NoopClassifier), evaluator, swallow_threshold)
    }

    /// Observe an outgoing tone — track its rid if it deserves an echo.
    pub fn sent(&self, tone: &Tone) {
        if matches!(tone.chi(), Chi::Drone | Chi::Echo) {
            return;
        }
        let Some(sigil) = tone.envelope.sigil.clone() else { return };
        let mut states = self.inner.states.write();
        let state = states.entry(sigil.clone()).or_insert_with(|| create_drone_state(sigil));
        if tracked(tone.chi()) {
            state.pending_echoes.insert(
                tone.rid().to_string(),
                PendingEcho {
                    rid: tone.rid().to_string(),
                    chi: chi_label(tone.chi()).to_string(),
                    time_ms: now_ms(),
                    retries: 0,
                },
            );
        }
    }

    /// Observe an incoming tone — clear echoes, sync wane, reset missed beats.
    pub fn heard(&self, tone: &Tone) {
        match tone.chi() {
            Chi::Echo => {
                let rid = tone.rid().to_string();
                let mut states = self.inner.states.write();
                for state in states.values_mut() {
                    if state.pending_echoes.remove(&rid).is_some() {
                        break;
                    }
                }
            }
            Chi::Drone => {
                let Some(sigil) = tone.envelope.sigil.clone() else { return };
                let mut states = self.inner.states.write();
                let state =
                    states.entry(sigil.clone()).or_insert_with(|| create_drone_state(sigil));
                state.last_beat_received = now_ms();
                state.missed_beats = 0;
                if let Some(w) = tone.envelope.wane {
                    state.remote_wane = w;
                }
            }
            _ => {
                let Some(sigil) = tone.envelope.sigil.clone() else { return };
                let mut states = self.inner.states.write();
                let state =
                    states.entry(sigil.clone()).or_insert_with(|| create_drone_state(sigil));
                // Process-death pulses reset per-process counters but leave the
                // cross-process channel state (wane, echoes, missed beats) alone.
                if tone.chi() == Chi::Pulse {
                    let kind = tone.body.get("kind").and_then(Value::as_str);
                    if matches!(kind, Some("roost-died" | "roost-evicted" | "roost-idle")) {
                        state.inflight_tools = 0;
                        state.response_text.clear();
                        state.suspicious = Suspicion::None;
                    }
                }
            }
        }
    }

    /// Observe an LLM-stream event surfaced by the nestler.
    pub fn observed(&self, sigil: &str, event: Observed) {
        let mut states = self.inner.states.write();
        let state = states
            .entry(sigil.to_string())
            .or_insert_with(|| create_drone_state(sigil.to_string()));
        match event {
            Observed::ToolStart { .. } => state.inflight_tools += 1,
            Observed::ToolEnd { .. } => {
                state.inflight_tools = state.inflight_tools.saturating_sub(1);
            }
            Observed::Tokens { delta } => {
                state.tokens_burned = state.tokens_burned.saturating_add(delta)
            }
            Observed::PermissionAsk => state.pending_permissions += 1,
            Observed::PermissionResolved => {
                state.pending_permissions = state.pending_permissions.saturating_sub(1);
            }
            Observed::TextDelta { text } => state.response_text.push_str(&text),
            Observed::TurnEnd => {
                if state.response_text.len() > 20 {
                    state.suspicious = self.inner.classifier.classify(&state.response_text);
                }
                // response_text stays until assess() consumes it — the optional
                // LLM judge wants to see it.
            }
        }
        state.health = derive_health(state);
        state.rhythm_ms = state.health.rhythm_ms();
    }

    /// Snapshot-and-judge. Returns the unified verdict the host steers on.
    ///
    /// This is the only place `Verdict::Swallow` can fire — and only if
    /// either the heuristic is `Critical` or the optional [`Evaluator`]
    /// agrees past the swallow threshold.
    pub fn assess(&self, sigil: &str) -> Assessment {
        let mut states = self.inner.states.write();
        let state = states
            .entry(sigil.to_string())
            .or_insert_with(|| create_drone_state(sigil.to_string()));
        state.health = derive_health(state);
        state.rhythm_ms = state.health.rhythm_ms();

        let mut verdict = Verdict::Ok;
        let mut reason = String::from("nominal");

        if state.missed_beats >= 3 {
            verdict = Verdict::Dead;
            reason = format!("missed {} beats", state.missed_beats);
        } else if state.local_wane != state.remote_wane && state.last_beat_received > 0 {
            verdict = Verdict::Drift;
            reason = format!("wane local={} remote={}", state.local_wane, state.remote_wane);
        } else {
            let now = now_ms();
            let deadline = state.rhythm_ms.saturating_mul(2) as i64;
            if state
                .pending_echoes
                .values()
                .any(|p| now - p.time_ms > deadline && p.retries < 3)
            {
                verdict = Verdict::Retry;
                reason = "echo timeout".into();
            }
        }

        // Suspicion is independent of channel health: a stream may be
        // "serene" by metrics while spewing a context-loss greeting.
        let suspicion_now = if state.response_text.len() > 20 {
            self.inner.classifier.classify(&state.response_text)
        } else {
            state.suspicious
        };

        if matches!(verdict, Verdict::Ok | Verdict::Retry) {
            let should_swallow = match suspicion_now {
                Suspicion::Critical => true,
                Suspicion::Heavy | Suspicion::Soft => {
                    if let Some(eval) = self.inner.evaluator.as_ref() {
                        eval.evaluate(&state.response_text, state)
                            >= self.inner.swallow_threshold
                    } else {
                        false
                    }
                }
                Suspicion::None => false,
            };
            if should_swallow {
                verdict = Verdict::Swallow;
                reason = format!("suspicion={:?}", suspicion_now);
            }
        }

        let raw = RawAssessment {
            sigil: sigil.to_string(),
            health: state.health,
            suspicion: suspicion_now,
            missed_beats: state.missed_beats,
            inflight_tools: state.inflight_tools,
            pending_permissions: state.pending_permissions,
            pending_echoes: state.pending_echoes.len() as u32,
            local_wane: state.local_wane,
            remote_wane: state.remote_wane,
            reason,
        };

        Assessment { unified: verdict, raw }
    }

    /// Bump the local wane after a tone is committed. Hosts call this
    /// from the same path that bumps the `WaneTracker`.
    pub fn set_local_wane(&self, sigil: &str, wane: u64) {
        let mut states = self.inner.states.write();
        let state = states
            .entry(sigil.to_string())
            .or_insert_with(|| create_drone_state(sigil.to_string()));
        state.local_wane = wane;
    }

    /// Mark a missed beat — the host's silence timer calls this when the
    /// remote didn't speak within `rhythm * 2`.
    pub fn mark_missed_beat(&self, sigil: &str) {
        let mut states = self.inner.states.write();
        let state = states
            .entry(sigil.to_string())
            .or_insert_with(|| create_drone_state(sigil.to_string()));
        state.missed_beats = state.missed_beats.saturating_add(1);
    }

    /// Bump retries on a pending echo. Host calls after re-sending.
    /// Returns the new retry count if the rid was tracked.
    pub fn note_retry(&self, sigil: &str, rid: &str) -> Option<u32> {
        let mut states = self.inner.states.write();
        let state = states.get_mut(sigil)?;
        let entry = state.pending_echoes.get_mut(rid)?;
        entry.retries = entry.retries.saturating_add(1);
        entry.time_ms = now_ms();
        Some(entry.retries)
    }

    /// Drop a sigil's state — the host calls this on `cleanup`.
    pub fn forget(&self, sigil: &str) {
        self.inner.states.write().remove(sigil);
    }

    /// Build the periodic beat payload. Caller wraps it in a tone.
    pub fn beat(&self, sigil: &str) -> DroneBeat {
        let mut states = self.inner.states.write();
        let active = states.len() as u32;
        let state = states
            .entry(sigil.to_string())
            .or_insert_with(|| create_drone_state(sigil.to_string()));
        state.last_beat_sent = now_ms();
        DroneBeat {
            sigil: sigil.to_string(),
            wane: state.local_wane,
            health: state.health,
            rhythm_ms: state.rhythm_ms,
            pending_echoes: state.pending_echoes.keys().cloned().collect(),
            load: BeatLoad {
                active_sigils: active,
                pending_permissions: state.pending_permissions,
                inflight_tools: state.inflight_tools,
                tokens_burned: state.tokens_burned,
            },
        }
    }

    /// Serialize a beat as a tone body — convenience for the host's
    /// thrum-send path.
    pub fn beat_body(beat: &DroneBeat) -> Map<String, Value> {
        match serde_json::to_value(beat) {
            Ok(Value::Object(m)) => m,
            _ => Map::new(),
        }
    }

    /// Snapshot a single sigil's state. Returns `None` if unknown.
    pub fn inspect(&self, sigil: &str) -> Option<DroneState> {
        self.inner.states.read().get(sigil).cloned()
    }
}

fn derive_health(state: &DroneState) -> Health {
    if state.missed_beats >= 3 {
        return Health::Critical;
    }
    if state.local_wane != state.remote_wane && state.last_beat_received > 0 {
        return Health::Critical;
    }
    let now = now_ms();
    let deadline = state.rhythm_ms.saturating_mul(2) as i64;
    if state.pending_echoes.values().any(|p| now - p.time_ms > deadline) {
        return Health::Critical;
    }

    if state.pending_permissions > 0
        || state.inflight_tools > 3
        || !state.pending_echoes.is_empty()
    {
        return Health::Tense;
    }

    if state.inflight_tools > 0 || state.tokens_burned > 0 {
        return Health::Alert;
    }

    Health::Serene
}

fn chi_label(chi: Chi) -> &'static str {
    match chi {
        Chi::Hello => "hello",
        Chi::Prompt => "prompt",
        Chi::Cancel => "cancel",
        Chi::Cleanup => "cleanup",
        Chi::Curate => "curate",
        Chi::ReleasePermit => "release-permit",
        Chi::TendrilResult => "tendril-result",
        Chi::ToolResult => "tool-result",
        Chi::PetalCell => "petal-cell",
        Chi::Breath => "breath",
        Chi::Chunk => "chunk",
        Chi::Finish => "finish",
        Chi::Error => "error",
        Chi::SessionReady => "session-ready",
        Chi::Pulse => "pulse",
        Chi::PermissionAsk => "permission-ask",
        Chi::TendrilReach => "tendril-reach",
        Chi::ToolCall => "tool-call",
        Chi::ToolMeta => "tool-meta",
        Chi::Echo => "echo",
        Chi::PerfMark => "perf-mark",
        Chi::Log => "log",
        Chi::Drone => "drone",
        Chi::DroneRetrofit => "drone-retrofit",
        Chi::PeerAdd => "peer-add",
        Chi::PeerRemove => "peer-remove",
        Chi::Attach => "attach",
        Chi::Detach => "detach",
        Chi::WaneSync => "wane-sync",
        Chi::GossipPublish => "gossip-publish",
        Chi::KadFindNode => "kad-find-node",
        Chi::KadFindNodeResp => "kad-find-node-resp",
        Chi::ToolInfo => "tool-info",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use thrum_core::Envelope;

    fn tone(chi: Chi, sigil: &str, rid: &str) -> Tone {
        let mut env = Envelope::new(chi, rid);
        env.sigil = Some(sigil.into());
        Tone::new(env)
    }

    #[test]
    fn fresh_sigil_is_serene_and_ok() {
        let d = Drone::new();
        let a = d.assess("s1");
        assert_eq!(a.unified, Verdict::Ok);
        assert_eq!(a.raw.health, Health::Serene);
    }

    #[test]
    fn missed_beats_go_dead() {
        let d = Drone::new();
        for _ in 0..3 {
            d.mark_missed_beat("s1");
        }
        let a = d.assess("s1");
        assert_eq!(a.unified, Verdict::Dead);
        assert_eq!(a.raw.health, Health::Critical);
    }

    #[test]
    fn tracked_prompt_creates_pending_echo() {
        let d = Drone::new();
        let t = tone(Chi::Prompt, "s1", "rid-1");
        d.sent(&t);
        let st = d.inspect("s1").unwrap();
        assert!(st.pending_echoes.contains_key("rid-1"));
    }

    #[test]
    fn incoming_echo_clears_pending() {
        let d = Drone::new();
        d.sent(&tone(Chi::Prompt, "s1", "rid-1"));
        let mut echo_env = Envelope::new(Chi::Echo, "rid-1");
        echo_env.sigil = Some("s1".into());
        d.heard(&Tone::new(echo_env));
        let st = d.inspect("s1").unwrap();
        assert!(st.pending_echoes.is_empty());
    }

    // Canned classifiers exercise the drone's branching logic without
    // depending on a particular pattern bank — pattern banks live in
    // nest-side crates (see `nest-common`).
    struct AlwaysCritical;
    impl Classifier for AlwaysCritical {
        fn classify(&self, _: &str) -> Suspicion { Suspicion::Critical }
    }
    struct AlwaysSoft;
    impl Classifier for AlwaysSoft {
        fn classify(&self, _: &str) -> Suspicion { Suspicion::Soft }
    }

    #[test]
    fn critical_suspicion_swallows_without_evaluator() {
        let d = Drone::with_classifier(Arc::new(AlwaysCritical));
        d.observed("s1", Observed::TextDelta { text: "anything past twenty characters".into() });
        d.observed("s1", Observed::TurnEnd);
        let a = d.assess("s1");
        assert_eq!(a.unified, Verdict::Swallow);
        assert_eq!(a.raw.suspicion, Suspicion::Critical);
    }

    #[test]
    fn soft_suspicion_alone_does_not_swallow() {
        let d = Drone::with_classifier(Arc::new(AlwaysSoft));
        d.observed("s1", Observed::TextDelta { text: "anything past twenty characters".into() });
        d.observed("s1", Observed::TurnEnd);
        let a = d.assess("s1");
        assert_ne!(a.unified, Verdict::Swallow);
        assert_eq!(a.raw.suspicion, Suspicion::Soft);
    }

    struct YesEvaluator;
    impl Evaluator for YesEvaluator {
        fn evaluate(&self, _text: &str, _state: &DroneState) -> f32 { 0.95 }
    }

    #[test]
    fn evaluator_promotes_soft_to_swallow() {
        let d = Drone::with_classifier_and_evaluator(
            Arc::new(AlwaysSoft),
            Arc::new(YesEvaluator),
            0.7,
        );
        d.observed("s1", Observed::TextDelta { text: "anything past twenty characters".into() });
        d.observed("s1", Observed::TurnEnd);
        let a = d.assess("s1");
        assert_eq!(a.unified, Verdict::Swallow);
    }

    #[test]
    fn noop_classifier_never_swallows() {
        let d = Drone::new(); // NoopClassifier
        d.observed(
            "s1",
            Observed::TextDelta {
                text: "I don't have any previous context — could you share more?".into(),
            },
        );
        d.observed("s1", Observed::TurnEnd);
        let a = d.assess("s1");
        assert_ne!(a.unified, Verdict::Swallow);
        assert_eq!(a.raw.suspicion, Suspicion::None);
    }

    #[test]
    fn wane_drift_yields_drift_verdict() {
        let d = Drone::new();
        let mut env = Envelope::new(Chi::Drone, "rid-b");
        env.sigil = Some("s1".into());
        env.wane = Some(5);
        d.heard(&Tone::new(env));
        d.set_local_wane("s1", 7);
        let a = d.assess("s1");
        assert_eq!(a.unified, Verdict::Drift);
    }

    #[test]
    fn pulse_death_resets_inflight() {
        let d = Drone::new();
        d.observed("s1", Observed::ToolStart { name: None });
        d.observed("s1", Observed::ToolStart { name: None });
        let mut env = Envelope::new(Chi::Pulse, "rid-p");
        env.sigil = Some("s1".into());
        let mut t = Tone::new(env);
        t.body.insert("kind".into(), Value::String("roost-died".into()));
        d.heard(&t);
        let st = d.inspect("s1").unwrap();
        assert_eq!(st.inflight_tools, 0);
    }
}
