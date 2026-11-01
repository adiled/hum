//! The thrum, as what claude hears, and what claude says, as the thrum.
//!
//! A cell is fed tones and gives tones back; claude, in stream-json, reads
//! user messages and writes events. [`heard`] turns a tone the cell is fed
//! into the line claude reads: a prompt's words and images as a user
//! message, the first of a session's carrying its system prompt at its
//! head; an answer to a tool claude called outside its sigil's tools as
//! that tool's result; a cancel as claude's own cancel request.
//!
//! claude takes a turn only from a user message, and resuming a transcript
//! that ends in tool results it first answers them with an empty "No
//! response requested." of its own, which [`CONTINUE`] disowns.
//!
//! [`Grower`] turns claude's events into tones. Its init is a `cell-ready`
//! pulse. Each content block unfurls as one petal by its index, text as
//! text and thinking as a sepal sealed by its signature; a tool use unfurls
//! nothing, since its call reaches through the nest's MCP. Its result wilts
//! the bloom with its usage as nectar, `max_tokens` when it ran out, or,
//! when claude flags it an error, errs with claude's reason, which claude
//! gives there and not on stderr.
//!
//! A bloom grown along a trellis answers in its shape and nothing else.
//! claude gives that answer through a tool of its own and may say words
//! around it, so such a bloom's words are held, and its result's
//! `structured_output` unfurls as its words, or the held words do when
//! there is none.

use std::collections::BTreeMap;

use serde_json::{Value, json};
use thrum_core::{Grain, Petal, PetalKind, PromptBody};

fn grain_block(grain: &Grain) -> Value {
    match (grain.kind.as_str(), &grain.data, &grain.url) {
        ("image", Some(data), _) => json!({
            "type": "image",
            "source": { "type": "base64", "media_type": grain.media_type, "data": data }
        }),
        ("image", None, Some(url)) => json!({
            "type": "image",
            "source": { "type": "url", "url": url }
        }),
        (kind, _, url) => {
            let at = url.as_deref().map(|u| format!(" ({u})")).unwrap_or_default();
            json!({ "type": "text", "text": format!("[attachment: kind={kind} media_type={}{at}]", grain.media_type) })
        }
    }
}

pub fn said(petals: &[Petal]) -> Vec<Value> {
    petals
        .iter()
        .filter_map(|p| match (p.kind.as_str(), &p.text, &p.grain) {
            ("text", Some(text), _) => Some(json!({ "type": "text", "text": text })),
            ("pollen", _, Some(grain)) => Some(grain_block(grain)),
            _ => None,
        })
        .collect()
}

pub const CONTINUE: &str = "Tool results came back after your turn ended. If a \"No response requested.\" sits between them and this line, the session added it when it resumed; you did not say it. Continue from where you left off.";

pub fn heard(tone: &Value, told: &mut Option<String>) -> Option<String> {
    match tone.get("chi").and_then(Value::as_str)? {
        "prompt" => {
            let body: PromptBody = serde_json::from_value(tone.clone()).ok()?;
            let system = body.system_prompt.filter(|s| !s.is_empty());
            let petals = body.content.filter(|c| !c.is_empty()).unwrap_or_else(|| {
                body.text.into_iter().map(|t| Petal { kind: PetalKind::Text.as_str().into(), text: Some(t), ..Petal::default() }).collect()
            });
            let mut content = said(&petals);
            if content.is_empty() {
                content.push(json!({ "type": "text", "text": CONTINUE }));
            }
            if let Some(system) = system
                && told.as_deref() != Some(system.as_str())
            {
                content.insert(0, json!({ "type": "text", "text": crate::instructions(&system) }));
                *told = Some(system);
            }
            Some(json!({ "type": "user", "message": { "role": "user", "content": content } }).to_string())
        }
        "tendril-result" => {
            let call_id = tone.get("callId").and_then(Value::as_str)?;
            let grasped = hum_nest::ToolResult::of(tone);
            Some(json!({
                "type": "user",
                "message": { "role": "user", "content": [{ "type": "tool_result", "tool_use_id": call_id, "content": grasped.output }] }
            }).to_string())
        }
        "cancel" => {
            let rid = tone.get("rid").and_then(Value::as_str)?;
            Some(json!({ "type": "control_cancel_request", "request_id": rid }).to_string())
        }
        _ => None,
    }
}

pub struct Grower {
    shaped: bool,
    open: BTreeMap<u64, PetalKind>,
    top: u64,
    held: String,
}

fn unfurl(petal: u64, kind: PetalKind, grew: Value) -> Value {
    let mut tone = json!({ "chi": "unfurl", "petal": petal, "kind": kind.as_str() });
    if let (Some(tone), Value::Object(grew)) = (tone.as_object_mut(), grew) {
        tone.extend(grew);
    }
    tone
}

impl Grower {
    pub fn new(shaped: bool) -> Self {
        Self { shaped, open: BTreeMap::new(), top: 0, held: String::new() }
    }

    fn shown(&self, kind: PetalKind) -> bool {
        kind != PetalKind::Tendril && !(self.shaped && kind == PetalKind::Text)
    }

    pub fn grew(&mut self, event: Value) -> Vec<Value> {
        let msg = if event.get("type").and_then(Value::as_str) == Some("stream_event") {
            event.get("event").cloned().unwrap_or(event)
        } else {
            event
        };
        let typ = msg.get("type").and_then(Value::as_str).unwrap_or("");
        let index = msg.get("index").and_then(Value::as_u64);
        match (typ, index) {
            ("system", _) if msg.get("subtype").and_then(Value::as_str) == Some("init") => vec![json!({
                "chi": "pulse",
                "kind": "cell-ready",
                "cell": msg.get("session_id").cloned().unwrap_or(Value::Null),
                "model": msg.get("model").cloned().unwrap_or(Value::Null),
                "tools": msg.get("tools").cloned().unwrap_or(json!([])),
            })],
            ("content_block_start", Some(petal)) => {
                let kind = match msg.pointer("/content_block/type").and_then(Value::as_str) {
                    Some("text") => PetalKind::Text,
                    Some("thinking" | "redacted_thinking") => PetalKind::Sepal,
                    Some("tool_use") => PetalKind::Tendril,
                    _ => return vec![],
                };
                self.open.insert(petal, kind);
                self.top = self.top.max(petal + 1);
                if self.shown(kind) { vec![unfurl(petal, kind, json!({}))] } else { vec![] }
            }
            ("content_block_delta", Some(petal)) => {
                let delta = msg.get("delta").cloned().unwrap_or(json!({}));
                match delta.get("type").and_then(Value::as_str).unwrap_or("") {
                    "text_delta" if self.shaped => {
                        self.held.push_str(delta.get("text").and_then(Value::as_str).unwrap_or(""));
                        vec![]
                    }
                    "text_delta" => vec![unfurl(petal, PetalKind::Text, json!({ "text": delta.get("text") }))],
                    "thinking_delta" => vec![unfurl(petal, PetalKind::Sepal, json!({ "text": delta.get("thinking") }))],
                    "signature_delta" => vec![unfurl(petal, PetalKind::Sepal, json!({ "propolis": delta.get("signature") }))],
                    _ => vec![],
                }
            }
            ("content_block_stop", Some(petal)) => match self.open.remove(&petal) {
                Some(kind) if self.shown(kind) => vec![unfurl(petal, kind, json!({ "closed": true }))],
                _ => vec![],
            },
            ("result", _) => {
                let mut tones = Vec::new();
                let subtype = msg.get("subtype").and_then(Value::as_str).unwrap_or("success");
                let nectar = msg.get("usage").cloned().unwrap_or(Value::Null);
                let failed = msg.get("is_error").and_then(Value::as_bool).unwrap_or(false) || subtype.starts_with("error");
                if failed {
                    let detail = msg.get("result").and_then(Value::as_str)
                        .or_else(|| msg.get("error").and_then(Value::as_str))
                        .filter(|s| !s.is_empty())
                        .unwrap_or("claude returned an error result with no detail (check auth / model / credit)");
                    tones.push(json!({ "chi": "error", "code": "nurse_error", "subtype": subtype, "message": detail, "nectar": nectar }));
                } else {
                    if self.shaped {
                        let held = std::mem::take(&mut self.held);
                        let words = msg.get("structured_output").filter(|v| !v.is_null()).map(Value::to_string).unwrap_or(held);
                        tones.push(unfurl(self.top, PetalKind::Text, json!({ "text": words })));
                        tones.push(unfurl(self.top, PetalKind::Text, json!({ "closed": true })));
                    }
                    let reason = match msg.get("stop_reason").and_then(Value::as_str) {
                        Some("max_tokens") => "max_tokens",
                        Some("refusal") => "refusal",
                        _ => "stop",
                    };
                    tones.push(json!({ "chi": "wilt", "reason": reason, "subtype": subtype, "nectar": nectar }));
                }
                self.open.clear();
                self.top = 0;
                self.held.clear();
                tones
            }
            _ => vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(v: Value) -> Value {
        json!({ "type": "stream_event", "event": v })
    }

    #[test]
    fn a_prompt_is_heard_as_a_user_message_told_its_system_prompt_when_it_changes() {
        let prompt = |system: &str| json!({
            "chi": "prompt", "sid": "s", "systemPrompt": system,
            "content": [
                { "kind": "text", "text": "what is this?" },
                { "kind": "pollen", "grain": { "kind": "image", "mediaType": "image/png", "data": "AAAA" } },
            ],
        });
        let mut told = None;
        let first: Value = serde_json::from_str(&heard(&prompt("be brief"), &mut told).unwrap()).unwrap();
        let parts = first["message"]["content"].as_array().unwrap();
        assert_eq!(crate::instructed(parts[0]["text"].as_str().unwrap()), Some("be brief"));
        assert_eq!(parts[1]["text"], "what is this?");
        assert_eq!(parts[2]["source"]["media_type"], "image/png");
        let again: Value = serde_json::from_str(&heard(&prompt("be brief"), &mut told).unwrap()).unwrap();
        assert_eq!(again["message"]["content"][0]["text"], "what is this?", "told once");
        let changed: Value = serde_json::from_str(&heard(&prompt("be terse"), &mut told).unwrap()).unwrap();
        assert_eq!(crate::instructed(changed["message"]["content"][0]["text"].as_str().unwrap()), Some("be terse"));
        assert_eq!(told.as_deref(), Some("be terse"));
    }

    #[test]
    fn a_prompt_with_no_words_is_heard_as_the_continue_line() {
        let mut none = None;
        let on: Value = serde_json::from_str(&heard(&json!({ "chi": "prompt", "sid": "s", "graft": [] }), &mut none).unwrap()).unwrap();
        assert_eq!(on["type"], "user");
        assert_eq!(on["message"]["content"], json!([{ "type": "text", "text": CONTINUE }]));
    }

    #[test]
    fn an_answer_and_a_cancel_are_heard_as_claude_says_them() {
        let mut none = None;
        let answer: Value = serde_json::from_str(&heard(&json!({ "chi": "tendril-result", "callId": "toolu_1", "output": "42" }), &mut none).unwrap()).unwrap();
        assert_eq!(answer["message"]["content"][0]["tool_use_id"], "toolu_1");
        assert_eq!(answer["message"]["content"][0]["content"], "42");
        let stop: Value = serde_json::from_str(&heard(&json!({ "chi": "cancel", "rid": "r-1" }), &mut none).unwrap()).unwrap();
        assert_eq!(stop, json!({ "type": "control_cancel_request", "request_id": "r-1" }));
    }

    #[test]
    fn claude_events_grow_as_petals_and_wilt_with_nectar() {
        let mut g = Grower::new(false);
        let mut tones = Vec::new();
        for e in [
            json!({ "type": "system", "subtype": "init", "session_id": "sess", "model": "m", "tools": [] }),
            ev(json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "thinking" } })),
            ev(json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "thinking_delta", "thinking": "hmm" } })),
            ev(json!({ "type": "content_block_stop", "index": 0 })),
            ev(json!({ "type": "content_block_start", "index": 1, "content_block": { "type": "tool_use" } })),
            ev(json!({ "type": "content_block_stop", "index": 1 })),
            ev(json!({ "type": "content_block_start", "index": 2, "content_block": { "type": "text" } })),
            ev(json!({ "type": "content_block_delta", "index": 2, "delta": { "type": "text_delta", "text": "hi" } })),
            ev(json!({ "type": "content_block_stop", "index": 2 })),
            json!({ "type": "result", "subtype": "success", "stop_reason": "max_tokens", "usage": { "input_tokens": 3 } }),
        ] {
            tones.extend(g.grew(e));
        }
        assert_eq!(tones[0]["kind"], "cell-ready");
        let kinds: Vec<&str> = tones.iter().filter(|t| t["chi"] == "unfurl").map(|t| t["kind"].as_str().unwrap()).collect();
        assert_eq!(kinds, ["sepal", "sepal", "sepal", "text", "text", "text"], "a tool use unfurls nothing");
        let wilt = tones.last().unwrap();
        assert_eq!(wilt["chi"], "wilt");
        assert_eq!(wilt["reason"], "max_tokens");
        assert_eq!(wilt["nectar"]["input_tokens"], 3);
    }

    #[test]
    fn a_shaped_bloom_answers_in_its_shape_alone() {
        let mut g = Grower::new(true);
        let mut tones = Vec::new();
        for e in [
            ev(json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "text" } })),
            ev(json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": "Here you go:" } })),
            ev(json!({ "type": "content_block_stop", "index": 0 })),
            json!({ "type": "result", "subtype": "success", "structured_output": { "color": "blue" } }),
        ] {
            tones.extend(g.grew(e));
        }
        let words: String = tones.iter().filter_map(|t| t["text"].as_str()).collect();
        assert_eq!(words, r#"{"color":"blue"}"#);
    }

    #[test]
    fn an_error_result_errs_with_claudes_reason() {
        let mut g = Grower::new(false);
        let tones = g.grew(json!({ "type": "result", "subtype": "success", "is_error": true, "result": "Not logged in" }));
        assert_eq!(tones[0]["chi"], "error");
        assert_eq!(tones[0]["message"], "Not logged in");
    }
}
