//! An Ollama conversation, as the blooms of a sigil.
//!
//! Ollama's chat API sends the whole conversation every time; hum keeps it
//! in a sigil, one bloom a turn. A conversation reads as blooms: each user
//! turn opens one, its words and images its content, and what the
//! assistant thought and said, the tools it called and what came back from
//! them are what grew in it. System words are the system prompt.
//!
//! A request's last bloom is what it asks for. Its words are the prompt's
//! content and the blooms before it the graft. A conversation that ends
//! with tool answers or with the assistant's own words asks for a bloom to
//! grow on: the prompt has no content, and every bloom is its graft.
//!
//! A tool call keeps the `id` the forager gave it when the client sends it
//! back. Most Ollama clients send it back without one, so a tool answer
//! with no `tool_call_id` answers the first call still unanswered whose
//! name is its `tool_name`, else the first still unanswered.
//!
//! A conversation keeps one sigil while it only grows. The sid a request
//! continues is the one its blooms so far were last grown in; one no one
//! has seen, one whose history was edited, or one already growing gets a
//! sigil of its own. The trace that finds it is the system prompt, the
//! words of every turn, and each tool call's name, arguments and answer,
//! since an Ollama client sends those back as it was given them and its
//! ids often not at all.

use lru::LruCache;
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::num::NonZeroUsize;
use thrum_core::{Bloom, Grain, Petal, PetalKind};

#[derive(Debug, Default, Deserialize)]
pub struct Message {
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub images: Option<Vec<String>>,
    #[serde(default)]
    pub thinking: Option<String>,
    #[serde(default)]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(default)]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ToolCall {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub function: Function,
}

#[derive(Debug, Default, Deserialize)]
pub struct Function {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub arguments: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tail {
    User,
    Tool,
    Assistant,
    None,
}

pub struct Said {
    pub system: Option<String>,
    pub blooms: Vec<Bloom>,
    pub tail: Tail,
}

#[derive(Debug, Clone)]
pub struct Answer {
    pub call_id: String,
    pub name: String,
    pub args: Value,
    pub result: String,
}

fn petal(kind: PetalKind) -> Petal {
    Petal { kind: kind.as_str().into(), ..Petal::default() }
}

pub fn text(words: &str) -> Petal {
    Petal { text: Some(words.into()), ..petal(PetalKind::Text) }
}

pub fn tendril(call_id: &str, name: &str, args: &Value) -> Petal {
    Petal { call_id: Some(call_id.into()), name: Some(name.into()), args: Some(args.clone()), ..petal(PetalKind::Tendril) }
}

fn is(p: &Petal, kind: PetalKind) -> bool {
    p.kind == kind.as_str()
}

pub fn image(data: &str) -> Petal {
    let media_type = match data.get(..4) {
        Some("iVBO") => "image/png",
        Some("/9j/") => "image/jpeg",
        Some("R0lG") => "image/gif",
        Some("UklG") => "image/webp",
        _ => "image/*",
    };
    let grain = Grain { kind: "image".into(), media_type: media_type.into(), data: Some(data.into()), url: None };
    Petal { grain: Some(grain), ..petal(PetalKind::Pollen) }
}

pub fn args_of(raw: Option<&Value>) -> Value {
    match raw {
        Some(Value::String(s)) => serde_json::from_str(s).unwrap_or_else(|_| Value::String(s.clone())),
        Some(Value::Null) | None => Value::Object(Default::default()),
        Some(v) => v.clone(),
    }
}

pub fn content_of(words: Option<&str>, images: Option<&[String]>) -> Vec<Petal> {
    let mut petals: Vec<Petal> = words.filter(|w| !w.is_empty()).map(text).into_iter().collect();
    petals.extend(images.unwrap_or(&[]).iter().map(|i| image(i)));
    petals
}

fn said(blooms: &mut Vec<Bloom>, petals: Vec<Petal>) {
    match blooms.last_mut() {
        Some(last) if last.grew.as_ref().is_none_or(Vec::is_empty) => last.content.get_or_insert_with(Vec::new).extend(petals),
        _ => blooms.push(Bloom { content: Some(petals), grew: Some(Vec::new()) }),
    }
}

fn grew(blooms: &mut Vec<Bloom>, petals: Vec<Petal>) {
    if blooms.is_empty() {
        blooms.push(Bloom { content: Some(Vec::new()), grew: Some(Vec::new()) });
    }
    if let Some(last) = blooms.last_mut() {
        last.grew.get_or_insert_with(Vec::new).extend(petals);
    }
}

fn unanswered(bloom: Option<&Bloom>, name: Option<&str>) -> String {
    let grew = bloom.and_then(|b| b.grew.as_deref()).unwrap_or(&[]);
    let answered = |id: &str| grew.iter().any(|p| is(p, PetalKind::Grasp) && p.call_id.as_deref() == Some(id));
    let open: Vec<&Petal> = grew
        .iter()
        .filter(|p| is(p, PetalKind::Tendril) && !answered(p.call_id.as_deref().unwrap_or("")))
        .collect();
    open.iter()
        .find(|p| name.is_some() && p.name.as_deref() == name)
        .or(open.first())
        .and_then(|p| p.call_id.clone())
        .unwrap_or_default()
}

pub fn from_chat(messages: &[Message]) -> Said {
    let mut system = Vec::new();
    let mut blooms = Vec::new();
    let mut tail = Tail::None;
    let mut calls = 0;
    for m in messages {
        match m.role.as_str() {
            "system" => system.push(m.content.clone().unwrap_or_default()),
            "user" => {
                said(&mut blooms, content_of(m.content.as_deref(), m.images.as_deref()));
                tail = Tail::User;
            }
            "assistant" => {
                let mut petals = Vec::new();
                if let Some(thought) = m.thinking.as_deref().filter(|t| !t.is_empty()) {
                    petals.push(Petal { text: Some(thought.into()), ..petal(PetalKind::Sepal) });
                }
                petals.extend(content_of(m.content.as_deref(), None));
                for c in m.tool_calls.as_deref().unwrap_or(&[]) {
                    calls += 1;
                    let id = c.id.clone().filter(|id| !id.is_empty()).unwrap_or_else(|| format!("call-{calls}"));
                    petals.push(tendril(&id, &c.function.name, &args_of(Some(&c.function.arguments))));
                }
                grew(&mut blooms, petals);
                tail = Tail::Assistant;
            }
            "tool" => {
                let call_id = m
                    .tool_call_id
                    .clone()
                    .filter(|id| !id.is_empty())
                    .unwrap_or_else(|| unanswered(blooms.last(), m.tool_name.as_deref()));
                let grasp = Petal {
                    call_id: Some(call_id),
                    result: Some(Value::String(m.content.clone().unwrap_or_default())),
                    ..petal(PetalKind::Grasp)
                };
                grew(&mut blooms, vec![grasp]);
                tail = Tail::Tool;
            }
            _ => {}
        }
    }
    let system = system.into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n\n");
    Said { system: (!system.is_empty()).then_some(system), blooms, tail }
}

pub fn opening(said: &Said) -> (Vec<Bloom>, Vec<Petal>) {
    match (said.tail, said.blooms.split_last()) {
        (Tail::User, Some((last, before))) => (before.to_vec(), last.content.clone().unwrap_or_default()),
        _ => (said.blooms.clone(), Vec::new()),
    }
}

pub fn answers(said: &Said) -> Vec<Answer> {
    if said.tail != Tail::Tool {
        return Vec::new();
    }
    let grew = said.blooms.last().and_then(|b| b.grew.as_deref()).unwrap_or(&[]);
    let called = |id: &str| grew.iter().find(|p| is(p, PetalKind::Tendril) && p.call_id.as_deref() == Some(id));
    let mut out: Vec<Answer> = grew
        .iter()
        .rev()
        .take_while(|p| is(p, PetalKind::Grasp))
        .map(|g| {
            let call_id = g.call_id.clone().unwrap_or_default();
            let call = called(&call_id);
            Answer {
                name: call.and_then(|c| c.name.clone()).unwrap_or_default(),
                args: call.and_then(|c| c.args.clone()).unwrap_or(Value::Null),
                result: match &g.result {
                    Some(Value::String(s)) => s.clone(),
                    Some(other) => other.to_string(),
                    None => String::new(),
                },
                call_id,
            }
        })
        .collect();
    out.reverse();
    out
}

fn squeeze(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn trace(system: Option<&str>, blooms: &[Bloom]) -> String {
    let mut h = Sha256::new();
    h.update(format!("{}\0", squeeze(system.unwrap_or(""))));
    for b in blooms {
        h.update("U");
        for p in b.content.as_deref().unwrap_or(&[]) {
            if is(p, PetalKind::Text) {
                h.update(format!("t{}\0", squeeze(p.text.as_deref().unwrap_or(""))));
            } else if is(p, PetalKind::Pollen) {
                h.update(format!("p{}\0", p.grain.as_ref().map_or("", |g| g.kind.as_str())));
            }
        }
        h.update("G");
        let mut words = String::new();
        let flush = |h: &mut Sha256, words: &mut String| {
            if !squeeze(words).is_empty() {
                h.update(format!("t{}\0", squeeze(words)));
            }
            words.clear();
        };
        for p in b.grew.as_deref().unwrap_or(&[]) {
            if is(p, PetalKind::Text) {
                words.push_str(p.text.as_deref().unwrap_or(""));
            } else if is(p, PetalKind::Tendril) {
                flush(&mut h, &mut words);
                let args = p.args.as_ref().map(Value::to_string).unwrap_or_default();
                h.update(format!("c{}\0{args}\0", p.name.as_deref().unwrap_or("")));
            } else if is(p, PetalKind::Grasp) {
                flush(&mut h, &mut words);
                let result = match &p.result {
                    Some(Value::String(s)) => squeeze(s),
                    Some(other) => other.to_string(),
                    None => String::new(),
                };
                h.update(format!("r{result}\0"));
            }
        }
        flush(&mut h, &mut words);
    }
    hex::encode(h.finalize())
}

pub struct Sigils {
    by_trace: LruCache<String, String>,
}

impl Sigils {
    pub fn new(cap: usize) -> Self {
        Self { by_trace: LruCache::new(NonZeroUsize::new(cap).unwrap_or(NonZeroUsize::MIN)) }
    }

    pub fn fresh() -> String {
        hum_identity::HumId::mint().to_string()
    }

    pub fn continuing(&mut self, system: Option<&str>, blooms: &[Bloom], growing: impl Fn(&str) -> bool) -> String {
        if blooms.is_empty() {
            return Self::fresh();
        }
        match self.by_trace.get(&trace(system, blooms)) {
            Some(sid) if !growing(sid) => sid.clone(),
            _ => Self::fresh(),
        }
    }

    pub fn remember(&mut self, system: Option<&str>, blooms: &[Bloom], sid: &str) -> Option<String> {
        let (_, gone) = self.by_trace.push(trace(system, blooms), sid.to_string())?;
        (!self.by_trace.iter().any(|(_, s)| *s == gone)).then_some(gone)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn chat(messages: Value) -> Said {
        from_chat(&serde_json::from_value::<Vec<Message>>(messages).unwrap())
    }

    #[test]
    fn a_conversation_reads_as_blooms() {
        let said = chat(json!([
            {"role": "system", "content": "be brief"},
            {"role": "user", "content": "weather?", "images": ["iVBORw0KGgo="]},
            {"role": "assistant", "content": "", "thinking": "look it up", "tool_calls": [{"function": {"name": "weather", "arguments": {"city": "Oslo"}}}]},
            {"role": "tool", "tool_name": "weather", "content": "snow"},
            {"role": "assistant", "content": "It snows."},
            {"role": "user", "content": "and tomorrow?"},
        ]));
        assert_eq!(said.system.as_deref(), Some("be brief"));
        assert_eq!(said.tail, Tail::User);
        let (graft, content) = opening(&said);
        assert_eq!(serde_json::to_value(&content).unwrap(), json!([{"kind": "text", "text": "and tomorrow?"}]));
        assert_eq!(serde_json::to_value(&graft).unwrap(), json!([{
            "content": [
                {"kind": "text", "text": "weather?"},
                {"kind": "pollen", "grain": {"kind": "image", "mediaType": "image/png", "data": "iVBORw0KGgo="}},
            ],
            "grew": [
                {"kind": "sepal", "text": "look it up"},
                {"kind": "tendril", "callId": "call-1", "name": "weather", "args": {"city": "Oslo"}},
                {"kind": "grasp", "callId": "call-1", "result": "snow"},
                {"kind": "text", "text": "It snows."},
            ],
        }]));
    }

    #[test]
    fn tool_answers_are_what_a_conversation_ending_in_them_asks() {
        let said = chat(json!([
            {"role": "user", "content": "two cities"},
            {"role": "assistant", "content": "", "tool_calls": [
                {"id": "a", "function": {"name": "weather", "arguments": "{\"city\":\"Oslo\"}"}},
                {"function": {"name": "time", "arguments": {}}},
            ]},
            {"role": "tool", "tool_name": "time", "content": "noon"},
            {"role": "tool", "tool_call_id": "a", "content": "snow"},
        ]));
        assert_eq!(said.tail, Tail::Tool);
        let (graft, content) = opening(&said);
        assert!(content.is_empty(), "it grows on");
        assert_eq!(graft.len(), 1);
        let answers: Vec<(String, String, Value, String)> =
            answers(&said).into_iter().map(|a| (a.call_id, a.name, a.args, a.result)).collect();
        assert_eq!(answers, [
            ("call-2".into(), "time".into(), json!({}), "noon".into()),
            ("a".into(), "weather".into(), json!({"city": "Oslo"}), "snow".into()),
        ]);
    }

    #[test]
    fn a_conversation_keeps_its_sigil_while_it_only_grows() {
        let mut sigils = Sigils::new(8);
        let one = chat(json!([{"role": "user", "content": "one"}]));
        let sid = sigils.continuing(one.system.as_deref(), &opening(&one).0, |_| false);
        let mut after = one.blooms.clone();
        after[0].grew = Some(vec![text("said  one"), tendril("x", "look", &json!({"b": 1, "a": 2}))]);
        assert_eq!(sigils.remember(None, &after, &sid), None);

        let two = chat(json!([
            {"role": "user", "content": "one"},
            {"role": "assistant", "content": "said one", "tool_calls": [{"function": {"name": "look", "arguments": {"a": 2, "b": 1}}}]},
            {"role": "user", "content": "two"},
        ]));
        let graft = opening(&two).0;
        assert_eq!(sigils.continuing(None, &graft, |_| false), sid, "ids and spacing aside, the same history");
        assert_ne!(sigils.continuing(None, &graft, |s| s == sid), sid, "unless it is growing");
        assert_ne!(sigils.continuing(Some("other"), &graft, |_| false), sid, "or its system prompt changed");
    }

    #[test]
    fn a_sigil_no_trace_leads_to_is_forgotten() {
        let mut sigils = Sigils::new(1);
        let blooms = |w: &str| vec![Bloom { content: Some(vec![text(w)]), grew: Some(vec![]) }];
        assert_eq!(sigils.remember(None, &blooms("a"), "s1"), None);
        assert_eq!(sigils.remember(None, &blooms("b"), "s2"), Some("s1".into()));
        assert_eq!(sigils.remember(None, &blooms("b"), "s2"), None, "the same trace again forgets nothing");
    }
}
