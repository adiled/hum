//! What a call answers with: JSON shown readably, narrowed by `--jq`, and
//! trimmed to `maxOutput` with both ends kept, as `bash` keeps a long
//! output's ends.

use jaq_core::load::{Arena, File, Loader};
use jaq_core::{Compiler, Ctx, Vars, data, unwrap_valr};
use jaq_json::Val;
use serde_json::Value;

pub fn json(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

pub fn jq(filter: &str, value: &Value) -> Result<String, String> {
    let input = jaq_json::read::parse_single(value.to_string().as_bytes()).map_err(|e| format!("--jq could not read the result: {e}"))?;
    let program = File { code: filter, path: () };
    let defs = jaq_core::defs().chain(jaq_std::defs()).chain(jaq_json::defs());
    let funs = jaq_core::funs().chain(jaq_std::funs()).chain(jaq_json::funs());
    let loader = Loader::new(defs);
    let arena = Arena::default();
    let modules = loader.load(&arena, program).map_err(|errs| format!("--jq {filter:?} does not parse: {}", describe(&errs)))?;
    let compiled = Compiler::default()
        .with_funs(funs)
        .compile(modules)
        .map_err(|errs| format!("--jq {filter:?} does not compile: {}", describe(&errs)))?;
    let ctx = Ctx::<data::JustLut<Val>>::new(&compiled.lut, Vars::new([]));
    let mut lines = Vec::new();
    for out in compiled.id.run((ctx, input)).map(unwrap_valr) {
        let val = out.map_err(|e| format!("--jq {filter:?} failed: {e}"))?;
        let text = val.to_string();
        lines.push(match serde_json::from_str::<Value>(&text) {
            Ok(Value::String(s)) => s,
            Ok(v) if v.is_object() || v.is_array() => json(&v),
            _ => text,
        });
    }
    Ok(lines.join("\n"))
}

fn describe<E: std::fmt::Debug>(errs: &E) -> String {
    let text = format!("{errs:?}");
    crate::catalogue::clip(&text, 300)
}

pub fn trim(text: String, max: usize) -> String {
    if text.len() <= max {
        return text;
    }
    let head_len = floor_char(&text, max * 3 / 4);
    let tail_start = ceil_char(&text, text.len() - max / 4);
    let cut = tail_start - head_len;
    format!(
        "{}\n… {cut} bytes cut here; narrow the result with --jq '<filter>' …\n{}",
        &text[..head_len],
        &text[tail_start..]
    )
}

fn floor_char(text: &str, mut at: usize) -> usize {
    while at > 0 && !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

fn ceil_char(text: &str, mut at: usize) -> usize {
    while at < text.len() && !text.is_char_boundary(at) {
        at += 1;
    }
    at
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn jq_narrows_a_result_as_jq_would() {
        let v = json!([{ "title": "a", "n": 1 }, { "title": "b", "n": 2 }]);
        assert_eq!(jq(".[].title", &v).unwrap(), "a\nb");
        assert_eq!(jq("map(.n) | add", &v).unwrap(), "3");
        assert_eq!(jq("map(select(.n > 1))", &v).unwrap(), json(&json!([{ "title": "b", "n": 2 }])));
        assert!(jq(".[", &v).unwrap_err().contains("does not parse"));
    }

    #[test]
    fn a_long_answer_keeps_both_ends_and_says_how_much_went() {
        let text: String = (0..2000).map(|i| format!("line {i}\n")).collect();
        let out = trim(text.clone(), 1000);
        assert!(out.len() < 1200);
        assert!(out.starts_with("line 0\n"));
        assert!(out.trim_end().ends_with("line 1999"));
        assert!(out.contains("bytes cut here"));
        assert_eq!(trim("short".into(), 1000), "short");
        let wide = "é".repeat(1000);
        let _ = trim(wide, 101);
    }
}
