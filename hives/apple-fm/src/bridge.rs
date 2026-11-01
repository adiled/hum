//! The bridge's wire: what one run of `apple-fm-gen` is asked, and what it
//! says back.
//!
//! A prompt becomes one JSON line on the bridge's stdin. The prompt's system
//! prompt becomes Apple's instructions, the conversation before it becomes
//! turns, each a prompt and the response it grew, and how the bloom grows
//! becomes Apple's generation options: `temperature`, `season` as the most
//! tokens it may grow, and `prune` and `seed` as how it samples, greedily
//! at a temperature of 0. The bridge answers in NDJSON, each line a
//! [`Said`]; its finish says how many of the oldest turns it dropped to fit
//! the model's context.
//!
//! Apple's model grows words from words. A prompt that carries pollen or
//! asks for a `trellis` is refused rather than grown as if it had not.

use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use thrum_core::{Bloom, Petal, PetalKind, PromptBody};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Turn {
    pub prompt: String,
    pub response: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum Sampling {
    Greedy,
    Top { k: u64, seed: Option<u64> },
    Nucleus { p: f64, seed: Option<u64> },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Ask {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    pub turns: Vec<Turn>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maximum_response_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sampling: Option<Sampling>,
    pub prompt: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "event", rename_all = "lowercase")]
pub enum Said {
    Delta {
        text: String,
    },
    Finish {
        reason: Option<String>,
        dropped: Option<u64>,
        #[serde(rename = "inputTokens")]
        input_tokens: Option<u64>,
        #[serde(rename = "outputTokens")]
        output_tokens: Option<u64>,
    },
    Error {
        code: String,
        message: String,
    },
    Probe {
        available: bool,
        reason: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Refusal {
    pub code: &'static str,
    pub message: String,
}

fn is(petal: &Petal, kind: PetalKind) -> bool {
    petal.kind == kind.as_str()
}

pub fn words(petals: &[Petal]) -> String {
    petals
        .iter()
        .filter(|p| is(p, PetalKind::Text))
        .filter_map(|p| p.text.as_deref())
        .collect::<Vec<_>>()
        .join("\n")
}

fn grown(petals: &[Petal]) -> String {
    petals
        .iter()
        .filter(|p| is(p, PetalKind::Text))
        .filter_map(|p| p.text.as_deref())
        .collect()
}

pub fn turns(graft: &[Bloom]) -> Vec<Turn> {
    graft
        .iter()
        .map(|bloom| Turn {
            prompt: words(bloom.content.as_deref().unwrap_or(&[])),
            response: grown(bloom.grew.as_deref().unwrap_or(&[])),
        })
        .filter(|turn| !turn.prompt.is_empty() || !turn.response.is_empty())
        .collect()
}

pub fn said_by(prompt: &PromptBody) -> String {
    let content = prompt.content.as_deref().unwrap_or(&[]);
    let said = words(content);
    if !said.is_empty() {
        return said;
    }
    prompt.text.clone().unwrap_or_default()
}

pub fn sampling(prompt: &PromptBody) -> Option<Sampling> {
    if prompt.temperature == Some(0.0) {
        return Some(Sampling::Greedy);
    }
    let seed = prompt.seed.map(|s| s as u64);
    let prune = prompt.prune.as_ref();
    if let Some(k) = prune.and_then(|p| p.k) {
        return Some(Sampling::Top { k, seed });
    }
    if let Some(p) = prune.and_then(|p| p.p) {
        return Some(Sampling::Nucleus { p, seed });
    }
    seed.map(|seed| Sampling::Nucleus { p: 1.0, seed: Some(seed) })
}

pub fn ask(prompt: &PromptBody, before: &[Turn]) -> Result<Ask, Refusal> {
    let content = prompt.content.as_deref().unwrap_or(&[]);
    if content.iter().any(|p| is(p, PetalKind::Pollen)) {
        return Err(Refusal {
            code: "apple_fm_words_only",
            message: "Apple's on-device model grows from words alone, and this prompt carries pollen".into(),
        });
    }
    if prompt.trellis.is_some() {
        return Err(Refusal {
            code: "apple_fm_no_trellis",
            message: "apple-fm grows free text, and cannot hold a bloom to a trellis".into(),
        });
    }
    let said = said_by(prompt);
    if said.is_empty() {
        return Err(Refusal { code: "apple_fm_no_words", message: "apple-fm grows a bloom only from new words".into() });
    }
    let sampling = sampling(prompt);
    Ok(Ask {
        instructions: prompt.system_prompt.clone().filter(|s| !s.is_empty()),
        turns: before.to_vec(),
        temperature: if sampling == Some(Sampling::Greedy) { None } else { prompt.temperature },
        maximum_response_tokens: prompt.season,
        sampling,
        prompt: said,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Availability {
    Available,
    Dormant(String),
    Never(String),
}

pub async fn probe(bridge: &Path) -> Result<Availability> {
    let run = tokio::process::Command::new(bridge).arg("--probe").kill_on_drop(true).output();
    let out = tokio::time::timeout(Duration::from_secs(30), run)
        .await
        .with_context(|| format!("{} --probe did not answer in 30s", bridge.display()))?
        .with_context(|| format!("run {} --probe", bridge.display()))?;
    let text = String::from_utf8_lossy(&out.stdout);
    let heard = text.lines().rev().find_map(|line| serde_json::from_str::<Said>(line).ok());
    match heard {
        Some(Said::Probe { available: true, .. }) => Ok(Availability::Available),
        Some(Said::Probe { reason, .. }) => {
            let reason = reason.unwrap_or_else(|| "unavailable".into());
            Ok(if reason == "device_not_eligible" { Availability::Never(reason) } else { Availability::Dormant(reason) })
        }
        _ => bail!(
            "{} --probe said no probe ({}): {}{}",
            bridge.display(),
            out.status,
            text.trim(),
            String::from_utf8_lossy(&out.stderr).trim()
        ),
    }
}

pub struct Frost {
    stops: Vec<String>,
    held: String,
}

impl Frost {
    pub fn new(stops: Option<&[String]>) -> Frost {
        let stops = stops.unwrap_or(&[]).iter().filter(|s| !s.is_empty()).cloned().collect();
        Frost { stops, held: String::new() }
    }

    pub fn pass(&mut self, text: &str) -> (String, bool) {
        if self.stops.is_empty() {
            return (text.to_string(), false);
        }
        self.held.push_str(text);
        if let Some(at) = self.stops.iter().filter_map(|s| self.held.find(s.as_str())).min() {
            let said = self.held[..at].to_string();
            self.held.clear();
            return (said, true);
        }
        let longest = self.stops.iter().map(String::len).max().unwrap_or(0);
        let from = self.held.len().saturating_sub(longest.saturating_sub(1));
        let cut = self
            .held
            .char_indices()
            .map(|(at, _)| at)
            .filter(|at| *at >= from)
            .find(|at| self.stops.iter().any(|s| s.starts_with(&self.held[*at..])))
            .unwrap_or(self.held.len());
        let said: String = self.held.drain(..cut).collect();
        (said, false)
    }

    pub fn thaw(&mut self) -> String {
        std::mem::take(&mut self.held)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn prompt(v: Value) -> PromptBody {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn a_graft_becomes_the_turns_before_the_prompt() {
        let p = prompt(json!({
            "chi": "prompt",
            "systemPrompt": "be brief",
            "content": [{"kind": "text", "text": "and now?"}],
            "graft": [
                {"content": [{"kind": "text", "text": "hi"}],
                 "grew": [{"kind": "sepal", "text": "hm"}, {"kind": "text", "text": "hello "}, {"kind": "text", "text": "there"}]},
                {"content": [{"kind": "text", "text": "name a fruit"}], "grew": [{"kind": "text", "text": "apple"}]},
                {"content": [], "grew": []},
            ],
        }));
        let before = turns(p.graft.as_deref().unwrap());
        assert_eq!(
            before,
            [
                Turn { prompt: "hi".into(), response: "hello there".into() },
                Turn { prompt: "name a fruit".into(), response: "apple".into() },
            ],
            "the words of each bloom, its thinking aside, and an empty bloom dropped"
        );
        let ask = ask(&p, &before).unwrap();
        assert_eq!(ask.instructions.as_deref(), Some("be brief"));
        assert_eq!(ask.prompt, "and now?");
        assert_eq!(ask.turns.len(), 2);
    }

    #[test]
    fn the_request_is_one_line_with_the_prompt_last() {
        let p = prompt(json!({"text": "say \"hi\"\non two lines", "season": 32, "temperature": 0.4}));
        let line = serde_json::to_string(&ask(&p, &[Turn { prompt: "a".into(), response: "b".into() }]).unwrap()).unwrap();
        assert!(!line.contains('\n'));
        assert_eq!(
            serde_json::from_str::<Value>(&line).unwrap(),
            json!({"turns": [{"prompt": "a", "response": "b"}], "temperature": 0.4, "maximumResponseTokens": 32, "prompt": "say \"hi\"\non two lines"})
        );
        assert!(line.ends_with(r#""prompt":"say \"hi\"\non two lines"}"#), "{line}");
    }

    #[test]
    fn how_a_bloom_grows_becomes_how_apple_samples() {
        let s = |v: Value| sampling(&prompt(v));
        assert_eq!(s(json!({"temperature": 0.0, "prune": {"k": 5}})), Some(Sampling::Greedy));
        assert_eq!(s(json!({"prune": {"k": 40, "p": 0.9}, "seed": 7})), Some(Sampling::Top { k: 40, seed: Some(7) }));
        assert_eq!(s(json!({"prune": {"p": 0.9}})), Some(Sampling::Nucleus { p: 0.9, seed: None }));
        assert_eq!(s(json!({"seed": 3})), Some(Sampling::Nucleus { p: 1.0, seed: Some(3) }));
        assert_eq!(s(json!({"temperature": 0.7})), None);

        let greedy = ask(&prompt(json!({"text": "x", "temperature": 0.0})), &[]).unwrap();
        assert_eq!(greedy.temperature, None, "greedy needs no temperature");
        assert_eq!(serde_json::to_value(&greedy.sampling).unwrap(), json!({"mode": "greedy"}));
        let top = serde_json::to_value(Sampling::Top { k: 40, seed: None }).unwrap();
        assert_eq!(top, json!({"mode": "top", "k": 40, "seed": null}));
    }

    #[test]
    fn pollen_a_trellis_and_no_words_are_refused() {
        let pollen = prompt(json!({"content": [
            {"kind": "text", "text": "what is this?"},
            {"kind": "pollen", "grain": {"kind": "image", "mediaType": "image/png", "data": "aGk="}},
        ]}));
        assert_eq!(ask(&pollen, &[]).unwrap_err().code, "apple_fm_words_only");
        let trellis = prompt(json!({"text": "json please", "trellis": "json"}));
        assert_eq!(ask(&trellis, &[]).unwrap_err().code, "apple_fm_no_trellis");
        assert_eq!(ask(&prompt(json!({"content": []})), &[]).unwrap_err().code, "apple_fm_no_words");
    }

    #[test]
    fn what_the_bridge_says_is_read() {
        let read = |s: &str| serde_json::from_str::<Said>(s).unwrap();
        assert_eq!(read(r#"{"text":"hi","event":"delta"}"#), Said::Delta { text: "hi".into() });
        assert_eq!(
            read(r#"{"event":"finish","reason":"max_tokens","dropped":2,"inputTokens":3,"outputTokens":2}"#),
            Said::Finish { reason: Some("max_tokens".into()), dropped: Some(2), input_tokens: Some(3), output_tokens: Some(2) }
        );
        assert_eq!(
            read(r#"{"event":"finish"}"#),
            Said::Finish { reason: None, dropped: None, input_tokens: None, output_tokens: None }
        );
        assert_eq!(
            read(r#"{"event":"error","code":"refusal","message":"no"}"#),
            Said::Error { code: "refusal".into(), message: "no".into() }
        );
        assert_eq!(
            read(r#"{"event":"probe","available":false,"reason":"model_not_ready"}"#),
            Said::Probe { available: false, reason: Some("model_not_ready".into()) }
        );
    }

    #[test]
    fn frost_holds_back_what_may_be_a_stop_and_ends_at_one() {
        let stops = ["THREE".to_string(), "\n\n".to_string()];
        let mut frost = Frost::new(Some(&stops));
        assert_eq!(frost.pass("one two TH"), ("one two ".into(), false), "TH may begin THREE");
        assert_eq!(frost.pass("REE four"), ("".into(), true), "a stop across two deltas is met");

        let mut frost = Frost::new(Some(&stops));
        assert_eq!(frost.pass("TH"), ("".into(), false));
        assert_eq!(frost.pass("ROW"), ("THROW".into(), false), "what turned out not to be a stop is said");
        assert_eq!(frost.pass("n\n"), ("n".into(), false));
        assert_eq!(frost.pass("x"), ("\nx".into(), false));
        assert_eq!(frost.pass("é\n"), ("é".into(), false), "held back on a char boundary");
        assert_eq!(frost.thaw(), "\n", "and what is held is said when the bloom finishes");

        let mut none = Frost::new(None);
        assert_eq!(none.pass("THREE"), ("THREE".into(), false));
        let mut empty = Frost::new(Some(&["".to_string()]));
        assert_eq!(empty.pass("x"), ("x".into(), false), "an empty stop stops nothing");
    }
}
