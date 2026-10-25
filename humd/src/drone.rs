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
//! flags anything; concrete classifiers live with the hives that host
//! cells (e.g. `hum-nest::RegexClassifier`).
//!
//! The verdict is what the dispatch loop branches on. `Ok` is nominal.
//! `Resync` means wane drifted and the host should resync state.
//! `TearDown` means missed beats passed the threshold and the channel
//! should come down. `Suspect` means a suspicious response was detected
//! and the host should retry the last bloom. `EchoTimeout` means the echo
//! never came back and the tracked tone should be re-sent.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use hum_nest::{Classifier, NoopClassifier, Suspicion};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thrum_core::{Chi, Tone};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Assessment {
    pub unified: Verdict,
    pub raw: RawAssessment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Verdict {
    Ok,
    Drift,
    Dead,
    Swallow,
    Retry,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawAssessment {
    pub sigil: String,
    pub health: Health,
    pub suspicion: Suspicion,
    pub missed_beats: u32,
    pub inflight_tools: u32,
    pub pending_echoes: u32,
    pub local_wane: u64,
    pub remote_wane: u64,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Health {
    Serene,
    Alert,
    Tense,
    Critical,
}

impl Health {
    pub fn rhythm_ms(self) -> u64 {
        match self {
            Health::Serene => 30_000,
            Health::Alert => 5_000,
            Health::Tense => 1_000,
            Health::Critical => 500,
        }
    }
}

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
        tokens_burned: 0,
        response_text: String::new(),
        suspicious: Suspicion::None,
    }
}

#[derive(Debug, Clone)]
pub enum Observed {
    TendrilReach {
        name: Option<String>,
    },
    TendrilResult {
        name: Option<String>,
    },
    Nectar {
        delta: u64,
    },
    Unfurl {
        text: String,
    },
    Wilt,
}

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
    pub inflight_tools: u32,
    pub tokens_burned: u64,
}

pub trait Evaluator: Send + Sync {
    fn evaluate(&self, text: &str, state: &DroneState) -> f32;
}

fn tracked(chi: Chi) -> bool {
    matches!(chi, Chi::Prompt | Chi::Cancel)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

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

    pub fn with_evaluator(evaluator: Arc<dyn Evaluator>, swallow_threshold: f32) -> Self {
        Self::with_classifier_and_evaluator(Arc::new(NoopClassifier), evaluator, swallow_threshold)
    }

    pub fn sent(&self, tone: &Tone) {
        if matches!(tone.chi(), Chi::Drone | Chi::Echo) {
            return;
        }
        let Some(sigil) = tone.envelope.sigil.clone() else {
            return;
        };
        let mut states = self.inner.states.write();
        let state = states
            .entry(sigil.clone())
            .or_insert_with(|| create_drone_state(sigil));
        if tracked(tone.chi()) {
            state.pending_echoes.insert(
                tone.rid().to_string(),
                PendingEcho {
                    rid: tone.rid().to_string(),
                    chi: tone.chi().as_str().to_string(),
                    time_ms: now_ms(),
                    retries: 0,
                },
            );
        }
    }

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
                let Some(sigil) = tone.envelope.sigil.clone() else {
                    return;
                };
                let mut states = self.inner.states.write();
                let state = states
                    .entry(sigil.clone())
                    .or_insert_with(|| create_drone_state(sigil));
                state.last_beat_received = now_ms();
                state.missed_beats = 0;
                if let Some(w) = tone.envelope.wane {
                    state.remote_wane = w;
                }
            }
            _ => {
                let Some(sigil) = tone.envelope.sigil.clone() else {
                    return;
                };
                let mut states = self.inner.states.write();
                let state = states
                    .entry(sigil.clone())
                    .or_insert_with(|| create_drone_state(sigil));
                if tone.chi() == Chi::Pulse {
                    let kind = tone.body.get("kind").and_then(Value::as_str);
                    if matches!(kind, Some("cell-died" | "cell-evicted" | "cell-idle")) {
                        state.inflight_tools = 0;
                        state.response_text.clear();
                        state.suspicious = Suspicion::None;
                    }
                }
            }
        }
    }

    pub fn observed(&self, sigil: &str, event: Observed) {
        let mut states = self.inner.states.write();
        let state = states
            .entry(sigil.to_string())
            .or_insert_with(|| create_drone_state(sigil.to_string()));
        match event {
            Observed::TendrilReach { .. } => state.inflight_tools += 1,
            Observed::TendrilResult { .. } => {
                state.inflight_tools = state.inflight_tools.saturating_sub(1);
            }
            Observed::Nectar { delta } => {
                state.tokens_burned = state.tokens_burned.saturating_add(delta)
            }
            Observed::Unfurl { text } => state.response_text.push_str(&text),
            Observed::Wilt => {
                if state.response_text.len() > 20 {
                    state.suspicious = self.inner.classifier.classify(&state.response_text);
                }
            }
        }
        state.health = derive_health(state);
        state.rhythm_ms = state.health.rhythm_ms();
    }

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
            reason = format!(
                "wane local={} remote={}",
                state.local_wane, state.remote_wane
            );
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
                        eval.evaluate(&state.response_text, state) >= self.inner.swallow_threshold
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
            pending_echoes: state.pending_echoes.len() as u32,
            local_wane: state.local_wane,
            remote_wane: state.remote_wane,
            reason,
        };

        Assessment {
            unified: verdict,
            raw,
        }
    }

    pub fn set_local_wane(&self, sigil: &str, wane: u64) {
        let mut states = self.inner.states.write();
        let state = states
            .entry(sigil.to_string())
            .or_insert_with(|| create_drone_state(sigil.to_string()));
        state.local_wane = wane;
    }

    pub fn mark_missed_beat(&self, sigil: &str) {
        let mut states = self.inner.states.write();
        let state = states
            .entry(sigil.to_string())
            .or_insert_with(|| create_drone_state(sigil.to_string()));
        state.missed_beats = state.missed_beats.saturating_add(1);
    }

    pub fn note_retry(&self, sigil: &str, rid: &str) -> Option<u32> {
        let mut states = self.inner.states.write();
        let state = states.get_mut(sigil)?;
        let entry = state.pending_echoes.get_mut(rid)?;
        entry.retries = entry.retries.saturating_add(1);
        entry.time_ms = now_ms();
        Some(entry.retries)
    }

    pub fn forget(&self, sigil: &str) {
        self.inner.states.write().remove(sigil);
    }

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
                inflight_tools: state.inflight_tools,
                tokens_burned: state.tokens_burned,
            },
        }
    }

    pub fn beat_body(beat: &DroneBeat) -> Map<String, Value> {
        match serde_json::to_value(beat) {
            Ok(Value::Object(m)) => m,
            _ => Map::new(),
        }
    }

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
    if state
        .pending_echoes
        .values()
        .any(|p| now - p.time_ms > deadline)
    {
        return Health::Critical;
    }

    if state.inflight_tools > 3 || !state.pending_echoes.is_empty() {
        return Health::Tense;
    }

    if state.inflight_tools > 0 || state.tokens_burned > 0 {
        return Health::Alert;
    }

    Health::Serene
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

    struct AlwaysCritical;
    impl Classifier for AlwaysCritical {
        fn classify(&self, _: &str) -> Suspicion {
            Suspicion::Critical
        }
    }
    struct AlwaysSoft;
    impl Classifier for AlwaysSoft {
        fn classify(&self, _: &str) -> Suspicion {
            Suspicion::Soft
        }
    }

    #[test]
    fn critical_suspicion_swallows_without_evaluator() {
        let d = Drone::with_classifier(Arc::new(AlwaysCritical));
        d.observed(
            "s1",
            Observed::Unfurl {
                text: "anything past twenty characters".into(),
            },
        );
        d.observed("s1", Observed::Wilt);
        let a = d.assess("s1");
        assert_eq!(a.unified, Verdict::Swallow);
        assert_eq!(a.raw.suspicion, Suspicion::Critical);
    }

    #[test]
    fn soft_suspicion_alone_does_not_swallow() {
        let d = Drone::with_classifier(Arc::new(AlwaysSoft));
        d.observed(
            "s1",
            Observed::Unfurl {
                text: "anything past twenty characters".into(),
            },
        );
        d.observed("s1", Observed::Wilt);
        let a = d.assess("s1");
        assert_ne!(a.unified, Verdict::Swallow);
        assert_eq!(a.raw.suspicion, Suspicion::Soft);
    }

    struct YesEvaluator;
    impl Evaluator for YesEvaluator {
        fn evaluate(&self, _text: &str, _state: &DroneState) -> f32 {
            0.95
        }
    }

    #[test]
    fn evaluator_promotes_soft_to_swallow() {
        let d =
            Drone::with_classifier_and_evaluator(Arc::new(AlwaysSoft), Arc::new(YesEvaluator), 0.7);
        d.observed(
            "s1",
            Observed::Unfurl {
                text: "anything past twenty characters".into(),
            },
        );
        d.observed("s1", Observed::Wilt);
        let a = d.assess("s1");
        assert_eq!(a.unified, Verdict::Swallow);
    }

    #[test]
    fn noop_classifier_never_swallows() {
        let d = Drone::new();
        d.observed(
            "s1",
            Observed::Unfurl {
                text: "I don't have any previous context — could you share more?".into(),
            },
        );
        d.observed("s1", Observed::Wilt);
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
        d.observed("s1", Observed::TendrilReach { name: None });
        d.observed("s1", Observed::TendrilReach { name: None });
        let mut env = Envelope::new(Chi::Pulse, "rid-p");
        env.sigil = Some("s1".into());
        let mut t = Tone::new(env);
        t.body
            .insert("kind".into(), Value::String("cell-died".into()));
        d.heard(&t);
        let st = d.inspect("s1").unwrap();
        assert_eq!(st.inflight_tools, 0);
    }
}
