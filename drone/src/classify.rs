//! Heuristic context-loss classifier — regex-driven, deterministic.
//!
//! The first gate. Suspicion levels flag — they do not decide. The LLM
//! judge (pluggable via [`Evaluator`](crate::Evaluator)) makes the call
//! when heuristics raise a flag.
//!
//! Ported from `drone/classify.ts`. Two semantic tiers collapsed into
//! four levels so callers can distinguish "raise threshold" from
//! "skip the LLM, swallow now".

use std::sync::OnceLock;

use regex::RegexSet;

/// How loud the heuristic is shouting.
///
/// - `None` — text looks fine
/// - `Soft` / `Heavy` — flagged for LLM-driven adjudication
/// - `Critical` — patterns near-certain mid-conversation; bypass the LLM
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Suspicion {
    None,
    Soft,
    Heavy,
    Critical,
}

impl Suspicion {
    /// True when any pattern matched — regardless of tier.
    pub fn flagged(self) -> bool {
        !matches!(self, Suspicion::None)
    }
}

// Critical: explicit context loss admission — the honest failure mode.
const CONTEXT_LOSS_EXPLICIT: &[&str] = &[
    r"(?i)\bi don'?t (have|see|recall|remember) (any )?(previous|prior|earlier|context|history|conversation)",
    r"(?i)\bno (previous|prior) (context|history|conversation|messages?)",
    r"(?i)\b(new|fresh|blank) (session|conversation|chat)\b",
    r"(?i)\bthere'?s nothing (before|prior|earlier)",
    r"(?i)\byour (first|very first) message",
    r"(?i)\bno (history|context) (available|found|stored|present)",
    r"(?i)\bI (can'?t|cannot) (access|see|view|read) (any )?(previous|prior|earlier)",
    r"(?i)\bthis (is|appears to be) (a |the )?(start|beginning) of (our|a|the) conversation",
    r"(?i)\bI (don'?t|do not) have (access to|visibility into|information about) (your |the |any )?(previous|prior)",
];

// Critical: identity reset — never legitimate after turn 1.
const IDENTITY_RESET: &[&str] = &[
    r"(?i)\bI'?m (OpenCode|Claude|an AI|a coding) ?(assistant|agent|language model|helper)?[.,!]",
    r"(?i)\b(best coding agent|software engineering tasks|Use the instructions below)",
    r"(?i)\bas an AI (language model|assistant|,? I)",
    r"(?i)\bI apologize.{0,30}(don'?t|cannot|can'?t) (have|access)",
];

// Critical: greeting reset — emoji greetings or "how can I help" mid-stream.
const GREETING_RESET: &[&str] = &[
    // The TS regex starts with `^.{0,20}(👋|Hey!|Hello!|Hi there).{0,30}(help|assist|can I)`.
    // Rust's `regex` does not support look-around but unicode literals are fine.
    r"(?i)^.{0,20}(\x{1F44B}|Hey!|Hello!|Hi there).{0,30}(help|assist|can I)",
    r"(?i)\bhow can I (help|assist) you( today| with)?\??",
    r"(?i)\bwhat (would you like|do you want|can I do|shall I) (me to |to )?(help|do|work)",
    r"(?i)\bI (can|could) help (you )?(with|by):\s*\n",
];

// Soft: compensation — searches/hedges instead of remembering.
const COMPENSATION: &[&str] = &[
    r"(?i)\b(let me|I'?ll) (search|look|check|scan|grep|find) (the |this |your |for )",
    r"(?i)\b(don'?t|do not|can'?t|cannot) (see|find) any (references?|mention|results?)\b",
    r"(?i)\bcould you (provide|give|share) (more|additional|some) (context|details|information)",
    r"(?i)\b(if|are) you (referring|asking|talking) (to|about)",
    r"(?i)\bcould (refer|mean|be referring) to",
    r"(?i)\b(not sure|unsure|unclear) what (you'?re|you are) (referring|asking|talking) (to|about)",
];

// Soft: formality shift — trust reset back to transactional mode.
const FORMALITY_SHIFT: &[&str] = &[
    r"(?i)\bI'?d (need|require) (more|additional) (information|context|details) (to|before|in order)",
    r"(?i)\bbefore I (proceed|continue|do that).{0,30}(confirm|sure|want)",
    r"(?i)\byou (might|may) want to (check|ask|verify|consult)",
    r"(?i)\b(not|isn'?t) (within|in) my (scope|primary|capabilities|focus)",
    r"(?i)\bI (want to|need to|should) (make sure|ensure|verify|confirm) (this is|you want|before)",
];

struct Bank {
    critical: RegexSet,
    // Heavy tier — identity / greeting reset. Same severity as critical in TS,
    // separated here so callers may distinguish admission from drift.
    heavy: RegexSet,
    soft: RegexSet,
}

fn bank() -> &'static Bank {
    static B: OnceLock<Bank> = OnceLock::new();
    B.get_or_init(|| {
        let crit_src: Vec<&str> = CONTEXT_LOSS_EXPLICIT.iter().copied().collect();
        let heavy_src: Vec<&str> =
            IDENTITY_RESET.iter().chain(GREETING_RESET.iter()).copied().collect();
        let soft_src: Vec<&str> =
            COMPENSATION.iter().chain(FORMALITY_SHIFT.iter()).copied().collect();
        Bank {
            critical: RegexSet::new(&crit_src).expect("drone: critical patterns compile"),
            heavy: RegexSet::new(&heavy_src).expect("drone: heavy patterns compile"),
            soft: RegexSet::new(&soft_src).expect("drone: soft patterns compile"),
        }
    })
}

/// Tiered classifier. Short-circuits at the loudest match.
pub fn classify_suspicion(text: &str) -> Suspicion {
    let b = bank();
    if b.critical.is_match(text) {
        return Suspicion::Critical;
    }
    if b.heavy.is_match(text) {
        return Suspicion::Heavy;
    }
    if b.soft.is_match(text) {
        return Suspicion::Soft;
    }
    Suspicion::None
}

/// Boolean form mirroring the TS `heuristicSuspicion` helper.
pub fn heuristic_suspicion(text: &str) -> bool {
    classify_suspicion(text).flagged()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_admission_is_critical() {
        assert_eq!(
            classify_suspicion("I don't have any previous context for this."),
            Suspicion::Critical
        );
        assert_eq!(
            classify_suspicion("This appears to be the start of our conversation."),
            Suspicion::Critical
        );
    }

    #[test]
    fn identity_reset_is_heavy() {
        assert_eq!(
            classify_suspicion("I'm Claude, an AI assistant."),
            Suspicion::Heavy
        );
    }

    #[test]
    fn compensation_is_soft() {
        assert_eq!(
            classify_suspicion("Let me search the codebase for that."),
            Suspicion::Soft
        );
    }

    #[test]
    fn benign_text_is_none() {
        assert_eq!(
            classify_suspicion("Done — the patch landed in lib/foo.rs and tests pass."),
            Suspicion::None
        );
    }

    #[test]
    fn flagged_helper_matches_tiers() {
        assert!(heuristic_suspicion("I don't recall any previous conversation"));
        assert!(!heuristic_suspicion("ok"));
    }
}
