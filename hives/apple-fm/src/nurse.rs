//! The nurse that grows blooms through Apple's on-device foundation model.
//!
//! Each prompt runs the bridge once: it is asked the prompt as one JSON line
//! and streams back what the model grows, and the words it grows unfurl as a
//! text petal. The bloom wilts `stop` when the bridge finishes, with
//! `max_tokens` as the reason when the prompt's `season` ran out, and its
//! counts as nectar when Apple gives them. A bloom that meets one of its
//! `frost` stops there: what came before the stop unfurls, and the bridge is
//! killed. The bridge's own errors, and a bridge that ends without a word,
//! become `error`s.
//!
//! A cell lives for its sigil and remembers what it grew. A prompt that
//! carries a graft is grown from that graft; one that carries none, as a
//! forager that sends only the new words does, is grown from the turns the
//! cell has grown so far. So a sigil holds its conversation either way. The
//! oldest turns the bridge dropped to fit the model's context, the cell
//! forgets too.
//!
//! A still kills the bridge mid-bloom and wilts the bloom `cancel`, and a
//! prompt still waiting is told it was not grown.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use hum_nest::{Cell, Egg, Nurse, Propensity};
use thrum_core::{PetalKind, PromptBody};

use crate::bridge::{self, Frost, Said, Turn};

pub struct AppleFm {
    pub bridge: PathBuf,
}

#[async_trait]
impl Nurse for AppleFm {
    fn ephemeral(&self) -> bool {
        false
    }

    fn propensity(&self) -> Propensity {
        Propensity::StatefulSession
    }

    async fn raise(&self, egg: Egg) -> Result<Cell> {
        info!(sid = %egg.sid, model = %egg.model_id, "apple-fm.raise");
        let bridge = self.bridge.clone();
        let (tx_feed, mut rx_feed) = mpsc::channel::<Value>(64);
        let (tx_mmm, rx_mmm) = mpsc::channel::<Value>(256);
        let (tx_exit, rx_exit) = oneshot::channel::<i32>();
        let silence = CancellationToken::new();
        let stilled = silence.clone();

        tokio::spawn(async move {
            let mut grew: Vec<Turn> = Vec::new();
            loop {
                let tone = tokio::select! {
                    biased;
                    _ = stilled.cancelled() => break,
                    tone = rx_feed.recv() => match tone {
                        Some(tone) => tone,
                        None => break,
                    },
                };
                if tone.get("chi").and_then(Value::as_str) != Some("prompt") {
                    continue;
                }
                let prompt: PromptBody = serde_json::from_value(tone).unwrap_or_default();
                let mut before = match prompt.graft.as_deref() {
                    Some(graft) if !graft.is_empty() => bridge::turns(graft),
                    _ => grew.clone(),
                };
                if let Some((turn, dropped)) = grow(&bridge, &prompt, &before, &tx_mmm, &stilled).await {
                    before.drain(..dropped.min(before.len()));
                    before.push(turn);
                    grew = before;
                }
            }
            rx_feed.close();
            while let Ok(tone) = rx_feed.try_recv() {
                if tone.get("chi").and_then(Value::as_str) == Some("prompt") {
                    let _ = tx_mmm.send(fail("apple_fm_cell_stilled", "the cell was stilled before it grew this prompt".into())).await;
                }
            }
            let _ = tx_exit.send(0);
        });

        Ok(Cell {
            mark: None,
            feed: tx_feed,
            mmm: Arc::new(Mutex::new(rx_mmm)),
            emerged: rx_exit,
            ephemeral: false,
            silence,
        })
    }
}

fn fail(code: &str, message: String) -> Value {
    json!({ "chi": "error", "code": code, "message": message })
}

struct Unfurler {
    tx: mpsc::Sender<Value>,
    petals: BTreeMap<&'static str, u64>,
}

impl Unfurler {
    async fn grow(&mut self, kind: PetalKind, text: &str) {
        if text.is_empty() {
            return;
        }
        let next = self.petals.len() as u64;
        let petal = *self.petals.entry(kind.as_str()).or_insert(next);
        let _ = self.tx.send(json!({ "chi": "unfurl", "petal": petal, "kind": kind.as_str(), "text": text })).await;
    }

    async fn close(&mut self) {
        for (kind, petal) in std::mem::take(&mut self.petals) {
            let _ = self.tx.send(json!({ "chi": "unfurl", "petal": petal, "kind": kind, "closed": true })).await;
        }
    }
}

fn tail(text: &str) -> &str {
    let text = text.trim();
    let from = text.len().saturating_sub(2000);
    let from = (from..=text.len()).find(|at| text.is_char_boundary(*at)).unwrap_or(text.len());
    &text[from..]
}

async fn grow(
    bridge: &Path,
    prompt: &PromptBody,
    before: &[Turn],
    tx: &mpsc::Sender<Value>,
    stilled: &CancellationToken,
) -> Option<(Turn, usize)> {
    let ask = match bridge::ask(prompt, before) {
        Ok(ask) => ask,
        Err(refusal) => {
            let _ = tx.send(fail(refusal.code, refusal.message)).await;
            return None;
        }
    };
    let mut child = match Command::new(bridge)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            let _ = tx.send(fail("apple_fm_bridge_spawn", format!("could not run the bridge at {}: {e}", bridge.display()))).await;
            return None;
        }
    };
    if let Some(mut stdin) = child.stdin.take() {
        let mut line = serde_json::to_vec(&ask).unwrap_or_default();
        line.push(b'\n');
        if let Err(e) = stdin.write_all(&line).await {
            warn!(err = %e, "apple-fm.bridge.stdin");
        }
    }
    let complaint = child.stderr.take().map(|mut stderr| {
        tokio::spawn(async move {
            let mut text = String::new();
            let _ = stderr.read_to_string(&mut text).await;
            text
        })
    });
    let Some(stdout) = child.stdout.take() else {
        let _ = tx.send(fail("apple_fm_bridge_spawn", "the bridge has no stdout".into())).await;
        return None;
    };
    let mut lines = BufReader::new(stdout).lines();
    let mut unfurler = Unfurler { tx: tx.clone(), petals: BTreeMap::new() };
    let mut frost = Frost::new(prompt.frost.as_deref());
    let mut said = String::new();
    let turn = |said: String| Turn { prompt: ask.prompt.clone(), response: said };

    loop {
        let line = tokio::select! {
            biased;
            _ = stilled.cancelled() => {
                let _ = child.kill().await;
                unfurler.close().await;
                let _ = tx.send(json!({ "chi": "wilt", "reason": "cancel" })).await;
                return None;
            }
            line = lines.next_line() => line,
        };
        let line = match line {
            Ok(Some(line)) => line,
            Ok(None) => break,
            Err(e) => {
                warn!(err = %e, "apple-fm.bridge.stdout");
                break;
            }
        };
        let heard = match serde_json::from_str::<Said>(&line) {
            Ok(heard) => heard,
            Err(e) => {
                warn!(err = %e, line = %line.chars().take(200).collect::<String>(), "apple-fm.bridge.unread");
                continue;
            }
        };
        match heard {
            Said::Delta { text } => {
                let (free, met) = frost.pass(&text);
                said.push_str(&free);
                unfurler.grow(PetalKind::Text, &free).await;
                if met {
                    let _ = child.kill().await;
                    unfurler.close().await;
                    let _ = tx.send(json!({ "chi": "wilt", "reason": "stop" })).await;
                    return Some((turn(said), 0));
                }
            }
            Said::Finish { reason, dropped, input_tokens, output_tokens } => {
                let dropped = dropped.unwrap_or(0) as usize;
                if dropped > 0 {
                    info!(dropped, kept = before.len().saturating_sub(dropped), "apple-fm.trimmed");
                }
                let thawed = frost.thaw();
                said.push_str(&thawed);
                unfurler.grow(PetalKind::Text, &thawed).await;
                unfurler.close().await;
                let ran_out = match reason.as_deref() {
                    Some("max_tokens") => true,
                    Some("stop") => false,
                    _ => matches!((prompt.season, output_tokens), (Some(season), Some(out)) if out >= season),
                };
                let mut wilt = json!({ "chi": "wilt", "reason": if ran_out { "max_tokens" } else { "stop" } });
                if input_tokens.is_some() || output_tokens.is_some() {
                    wilt["nectar"] = json!({
                        "input_tokens": input_tokens.unwrap_or(0),
                        "output_tokens": output_tokens.unwrap_or(0),
                    });
                }
                let _ = tx.send(wilt).await;
                let _ = child.kill().await;
                return Some((turn(said), dropped));
            }
            Said::Error { code, message } => {
                unfurler.close().await;
                let _ = tx.send(fail(&format!("apple_fm_{code}"), message)).await;
                let _ = child.kill().await;
                return None;
            }
            Said::Probe { .. } => {}
        }
    }

    let status = match tokio::time::timeout(Duration::from_secs(2), child.wait()).await {
        Ok(status) => status.map(|s| s.to_string()).unwrap_or_else(|e| e.to_string()),
        Err(_) => {
            let _ = child.kill().await;
            "it closed its stdout and lived on, so it was killed".to_string()
        }
    };
    let complaint = match complaint {
        Some(task) => tokio::time::timeout(Duration::from_secs(2), task).await.ok().and_then(Result::ok).unwrap_or_default(),
        None => String::new(),
    };
    unfurler.close().await;
    let _ = tx
        .send(fail("apple_fm_bridge_died", format!("the bridge ended before it finished ({status}): {}", tail(&complaint))))
        .await;
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const STUB: &str = r#"#!/bin/sh
here="$(cd "$(dirname "$0")" && pwd)"
IFS= read -r line
printf '%s\n' "$line" >> "$here/asked"
echo $$ > "$here/pid"
words=$(printf '%s' "$line" | sed -n 's/.*"prompt":"\([^"]*\)"}$/\1/p')
case "$words" in
  boom*) echo '{"event":"error","code":"guardrail_violation","message":"the guardrail held"}'; exit 1 ;;
  die*) echo 'the bridge fell over' >&2; exit 3 ;;
  hang*) echo '{"event":"delta","text":"hanging"}'; exec sleep 30 ;;
  frost*) echo '{"event":"delta","text":"one two TH"}'; echo '{"event":"delta","text":"REE four"}'; exec sleep 30 ;;
  long*) echo '{"event":"delta","text":"on and on"}'; echo '{"event":"finish","inputTokens":4,"outputTokens":8}'; exit 0 ;;
  capped*) echo '{"event":"delta","text":"cut"}'; echo '{"event":"finish","reason":"max_tokens","outputTokens":3}'; exit 0 ;;
  trim*) echo '{"event":"delta","text":"trimmed"}'; echo '{"event":"finish","reason":"stop","dropped":2}'; exit 0 ;;
  linger*) echo '{"event":"delta","text":"lingering"}'; echo '{"event":"finish"}'; exec sleep 30 ;;
  mute*) echo '{"event":"delta","text":"muted"}'; exec 1>&-; exec sleep 30 ;;
esac
echo '{"event":"delta","text":"heard "}'
printf '{"event":"delta","text":"%s. "}\n' "$words"
echo '{"event":"finish","inputTokens":7,"outputTokens":3}'
"#;

    struct Stub {
        dir: tempfile::TempDir,
    }

    impl Stub {
        fn new() -> Stub {
            use std::os::unix::fs::PermissionsExt;
            let dir = tempfile::tempdir().unwrap();
            let bridge = dir.path().join("apple-fm-gen");
            std::fs::write(&bridge, STUB).unwrap();
            std::fs::set_permissions(&bridge, std::fs::Permissions::from_mode(0o755)).unwrap();
            Stub { dir }
        }

        fn nurse(&self) -> AppleFm {
            AppleFm { bridge: self.dir.path().join("apple-fm-gen") }
        }

        fn asked(&self) -> Vec<Value> {
            std::fs::read_to_string(self.dir.path().join("asked"))
                .unwrap_or_default()
                .lines()
                .map(|l| serde_json::from_str(l).unwrap())
                .collect()
        }

        fn alive(&self) -> bool {
            let pid = std::fs::read_to_string(self.dir.path().join("pid")).unwrap();
            std::process::Command::new("kill").args(["-0", pid.trim()]).stderr(std::process::Stdio::null()).status().unwrap().success()
        }
    }

    fn fed(words: &str) -> Value {
        json!({"chi": "prompt", "sid": "s", "content": [{"kind": "text", "text": words}]})
    }

    async fn raised(stub: &Stub) -> Cell {
        let egg = Egg::new(hum_identity::HumId::mint(), "apple-3b-on-device", "/tmp");
        stub.nurse().raise(egg).await.unwrap()
    }

    async fn closing(cell: &Cell, blooms: usize) -> Vec<Value> {
        let mut mmm = cell.mmm.lock().await;
        let mut heard = Vec::new();
        while heard.iter().filter(|t: &&Value| t["chi"] == "wilt" || t["chi"] == "error").count() < blooms {
            let tone = tokio::time::timeout(Duration::from_secs(10), mmm.recv()).await;
            heard.push(tone.expect("the cell answers").expect("the cell lives on"));
        }
        heard
    }

    fn said(tones: &[Value]) -> String {
        tones.iter().filter(|t| t["chi"] == "unfurl").filter_map(|t| t["text"].as_str()).collect()
    }

    fn ends(tones: &[Value]) -> Vec<Value> {
        tones.iter().filter(|t| t["chi"] == "wilt" || t["chi"] == "error").cloned().collect()
    }

    #[tokio::test]
    async fn a_prompt_unfurls_and_wilts_with_its_nectar() {
        let stub = Stub::new();
        let cell = raised(&stub).await;
        cell.feed.send(fed("hello")).await.unwrap();
        let tones = closing(&cell, 1).await;
        assert_eq!(said(&tones), "heard hello. ");
        assert_eq!(
            tones.iter().filter(|t| t["chi"] == "unfurl").map(|t| (t["petal"].clone(), t["closed"].clone())).collect::<Vec<_>>(),
            [(json!(0), Value::Null), (json!(0), Value::Null), (json!(0), json!(true))],
            "one text petal, closed before the wilt"
        );
        assert_eq!(ends(&tones), [json!({"chi": "wilt", "reason": "stop", "nectar": {"input_tokens": 7, "output_tokens": 3}})]);
    }

    #[tokio::test]
    async fn a_cell_remembers_its_turns_and_a_graft_overrides_them() {
        let stub = Stub::new();
        let cell = raised(&stub).await;
        let mut first = fed("one");
        first["systemPrompt"] = json!("be brief");
        cell.feed.send(first).await.unwrap();
        cell.feed.send(fed("two")).await.unwrap();
        assert_eq!(said(&closing(&cell, 2).await), "heard one. heard two. ");
        let graft = json!([{"content": [{"kind": "text", "text": "elsewhere"}], "grew": [{"kind": "text", "text": "then"}]}]);
        let mut grafted = fed("three");
        grafted["graft"] = graft;
        cell.feed.send(grafted).await.unwrap();
        cell.feed.send(fed("four")).await.unwrap();
        closing(&cell, 2).await;

        let asked = stub.asked();
        assert_eq!(asked[0], json!({"instructions": "be brief", "turns": [], "prompt": "one"}));
        assert_eq!(asked[1]["turns"], json!([{"prompt": "one", "response": "heard one. "}]), "the cell's own turn");
        assert_eq!(asked[2]["turns"], json!([{"prompt": "elsewhere", "response": "then"}]), "the graft, not the cell's turns");
        assert_eq!(
            asked[3]["turns"],
            json!([{"prompt": "elsewhere", "response": "then"}, {"prompt": "three", "response": "heard three. "}]),
            "and the cell grows on from the graft"
        );
    }

    #[tokio::test]
    async fn a_still_kills_the_bridge_and_closes_what_it_was_fed() {
        let stub = Stub::new();
        let cell = raised(&stub).await;
        cell.feed.send(fed("hang on")).await.unwrap();
        cell.feed.send(fed("waiting")).await.unwrap();
        {
            let mut mmm = cell.mmm.lock().await;
            let first = tokio::time::timeout(Duration::from_secs(10), mmm.recv()).await.unwrap().unwrap();
            assert_eq!(first["text"], "hanging");
        }
        assert!(stub.alive());
        cell.still();
        let tones = closing(&cell, 2).await;
        let ends: Vec<String> = ends(&tones)
            .iter()
            .map(|t| format!("{} {}", t["chi"].as_str().unwrap(), t["reason"].as_str().or(t["code"].as_str()).unwrap()))
            .collect();
        assert_eq!(ends, ["wilt cancel", "error apple_fm_cell_stilled"], "the growing bloom wilts, the waiting one is told");
        assert!(!stub.alive(), "the bridge is killed, not left to grow");
        assert!(cell.mmm.lock().await.recv().await.is_none(), "then the cell ends");
        assert_eq!(cell.emerged.await, Ok(0));
    }

    #[tokio::test]
    async fn a_stop_ends_the_bloom_before_it_and_kills_the_bridge() {
        let stub = Stub::new();
        let cell = raised(&stub).await;
        let mut frosted = fed("frost");
        frosted["frost"] = json!(["THREE"]);
        cell.feed.send(frosted).await.unwrap();
        let tones = closing(&cell, 1).await;
        assert_eq!(said(&tones), "one two ");
        assert_eq!(ends(&tones), [json!({"chi": "wilt", "reason": "stop"})]);
        assert!(!stub.alive());
        cell.feed.send(fed("after")).await.unwrap();
        closing(&cell, 1).await;
        assert_eq!(stub.asked()[1]["turns"], json!([{"prompt": "frost", "response": "one two "}]), "the turn is what was said");
    }

    #[tokio::test]
    async fn a_season_that_ran_out_wilts_max_tokens() {
        let stub = Stub::new();
        let cell = raised(&stub).await;
        let mut seasoned = fed("long");
        seasoned["season"] = json!(8);
        cell.feed.send(seasoned).await.unwrap();
        let tones = closing(&cell, 1).await;
        assert_eq!(ends(&tones)[0]["reason"], "max_tokens");
        assert_eq!(stub.asked()[0]["maximumResponseTokens"], 8);
        let mut roomy = fed("long");
        roomy["season"] = json!(9);
        cell.feed.send(roomy).await.unwrap();
        assert_eq!(ends(&closing(&cell, 1).await)[0]["reason"], "stop");
    }

    #[tokio::test]
    async fn the_bridge_s_own_reason_is_the_wilt_s() {
        let stub = Stub::new();
        let cell = raised(&stub).await;
        cell.feed.send(fed("capped")).await.unwrap();
        assert_eq!(ends(&closing(&cell, 1).await)[0]["reason"], "max_tokens", "a reply the bridge held to the room left ran out");
    }

    #[tokio::test]
    async fn the_turns_the_bridge_dropped_the_cell_forgets() {
        let stub = Stub::new();
        let cell = raised(&stub).await;
        for words in ["one", "two", "three", "trim now", "after"] {
            cell.feed.send(fed(words)).await.unwrap();
        }
        closing(&cell, 5).await;
        let asked = stub.asked();
        assert_eq!(asked[3]["turns"].as_array().unwrap().len(), 3);
        assert_eq!(
            asked[4]["turns"],
            json!([{"prompt": "three", "response": "heard three. "}, {"prompt": "trim now", "response": "trimmed"}]),
            "the two oldest turns are gone"
        );
    }

    #[tokio::test]
    async fn a_bridge_that_lingers_after_its_finish_does_not_hold_the_cell() {
        let stub = Stub::new();
        let cell = raised(&stub).await;
        cell.feed.send(fed("linger")).await.unwrap();
        cell.feed.send(fed("next")).await.unwrap();
        let tones = closing(&cell, 2).await;
        assert_eq!(said(&tones), "lingeringheard next. ");
    }

    #[tokio::test]
    async fn a_bridge_that_falls_silent_but_lives_on_is_killed() {
        let stub = Stub::new();
        let cell = raised(&stub).await;
        cell.feed.send(fed("mute")).await.unwrap();
        cell.feed.send(fed("next")).await.unwrap();
        let tones = closing(&cell, 2).await;
        let ends = ends(&tones);
        assert_eq!(ends[0]["code"], "apple_fm_bridge_died");
        assert!(ends[0]["message"].as_str().unwrap().contains("killed"), "{}", ends[0]);
        assert_eq!(ends[1]["reason"], "stop", "and the cell grows on");
    }

    #[tokio::test]
    async fn what_goes_wrong_is_told_and_the_cell_grows_on() {
        let stub = Stub::new();
        let cell = raised(&stub).await;
        cell.feed.send(fed("boom")).await.unwrap();
        cell.feed.send(fed("die now")).await.unwrap();
        cell.feed.send(json!({"chi": "prompt", "sid": "s", "text": "look", "trellis": "json"})).await.unwrap();
        cell.feed.send(fed("fine")).await.unwrap();
        let tones = closing(&cell, 4).await;
        let ends = ends(&tones);
        assert_eq!(ends[0], json!({"chi": "error", "code": "apple_fm_guardrail_violation", "message": "the guardrail held"}));
        assert_eq!(ends[1]["code"], "apple_fm_bridge_died");
        let died = ends[1]["message"].as_str().unwrap();
        assert!(died.contains("exit status: 3") && died.contains("the bridge fell over"), "{died}");
        assert_eq!(ends[2]["code"], "apple_fm_no_trellis");
        assert_eq!(ends[3]["reason"], "stop");
        assert_eq!(stub.asked().len(), 3, "the refused prompt never ran the bridge");
        assert_eq!(stub.asked()[2]["turns"], json!([]), "a bloom that failed is no turn");
    }

    #[tokio::test]
    async fn a_missing_bridge_is_told() {
        let nurse = AppleFm { bridge: "/nonexistent/apple-fm-gen".into() };
        let cell = nurse.raise(Egg::new(hum_identity::HumId::mint(), "m", "/tmp")).await.unwrap();
        cell.feed.send(fed("hi")).await.unwrap();
        assert_eq!(ends(&closing(&cell, 1).await)[0]["code"], "apple_fm_bridge_spawn");
    }
}
