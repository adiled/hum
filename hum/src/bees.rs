//! Bees: the participants a humd hears from.
//!
//! hum starts no bee. A hive enters one, minting its key and signing it
//! in under the hive by a label of its choosing when it wants to find the
//! same bee again, and whatever acts as that bee signs its hello with the
//! key. Exiting signs it out; humd refuses its next hello.
//!
//! What humd advised a bee (WIRE.md, Advisories) is listed with it, and
//! `hum doctor` says what each advisory means.

use anyhow::{Result, bail};
use hum_identity::Hid;
use serde_json::Value;

fn nestled() -> Vec<Value> {
    std::fs::read_to_string(hum_paths::bees_snapshot())
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v.as_object().map(|o| o.values().cloned().collect()))
        .unwrap_or_default()
}

fn text(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or("").to_string()
}

fn texts(v: &Value, k: &str) -> Vec<String> {
    v.get(k)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default()
}

pub fn enter(hive: &str, label: Option<&str>, role: thrum_core::Role) -> Result<()> {
    let key = match label {
        Some(label) => hum_identity::enter_as(hive, label, role.as_str())?,
        None => {
            if !hum_paths::hive_installed(hive) {
                bail!("{hive} is not installed on this humd — `hum hive <ref> install` first");
            }
            let key = hum_identity::mint_bee_key()?;
            hum_identity::enter(hive, "", role.as_str(), &key)?;
            key
        }
    };
    println!("{}", key.id());
    Ok(())
}

pub fn act(id: &str, verb: &str) -> Result<()> {
    match verb {
        "exit" => {
            if hum_identity::exit(id)? {
                println!("{id} exited; humd refuses its next hello");
            } else {
                println!("{id} had already exited");
            }
        }
        "reenter" => {
            if hum_identity::reenter(id)? {
                println!("{id} reentered");
            } else {
                println!("{id} had not exited");
            }
        }
        "enter" => bail!("a bee is entered under a hive, not by id: hum bee enter --hive <hive> --role <role>"),
        other => bail!("unknown verb '{other}' (exit | reenter)"),
    }
    Ok(())
}

fn ago(ms: i64) -> String {
    let secs = (thrum_core::now_ms() - ms).max(0) / 1000;
    match secs {
        s if s < 120 => format!("{s}s ago"),
        s if s < 7200 => format!("{}m ago", s / 60),
        s if s < 172_800 => format!("{}h ago", s / 3600),
        s => format!("{}d ago", s / 86_400),
    }
}

pub fn advice() {
    let book = humd::advise::read(&hum_paths::advisories());
    if book.is_empty() {
        println!("  none: every bee speaks the thrum as it declares");
        return;
    }
    for (id, page) in &book {
        let short: String = id.chars().take(16).collect();
        for e in &page.advisories {
            let times = if e.count > 1 { format!(", {} times", e.count) } else { String::new() };
            println!("  {} {short}  {}: {} ({}{times})", page.hive, e.code, e.message, ago(e.last));
        }
    }
}

pub fn list() -> Result<()> {
    let entered = hum_identity::entered();
    let nestled = nestled();
    let book = humd::advise::read(&hum_paths::advisories());
    if entered.is_empty() && nestled.is_empty() {
        println!("no bee has entered this humd.");
        println!("a hive enters one: hum bee enter --hive <hive> --role <role>");
        return Ok(());
    }

    let nest_of = |id: &str| {
        let hid = Hid::from_hex(id).ok()?;
        nestled
            .iter()
            .find(|m| Hid::from_hex(&text(m, "hid")).is_ok_and(|h| h == hid))
    };

    for (id, entry) in &entered {
        let state = match (entry.exited, nest_of(id)) {
            (true, _) => "exited",
            (false, Some(_)) => "nestled",
            (false, None) => "entered, not nestled",
        };
        let mark = if state == "nestled" { "●" } else { "○" };
        let label = if entry.label.is_empty() { String::new() } else { format!(" as {}", entry.label) };
        let role = if entry.role.is_empty() { String::new() } else { format!(", {}", entry.role) };
        println!("{mark} {id}  —  hive {}{label}{role}, {state}", entry.hive);
        if let Some(m) = nest_of(id) {
            let models = texts(m, "models");
            if !models.is_empty() {
                println!("    models:   {}", models.join(", "));
            }
            let tools: Vec<String> = m
                .get("tools")
                .and_then(Value::as_array)
                .map(|a| a.iter().map(|t| text(t, "name")).filter(|n| !n.is_empty()).collect())
                .unwrap_or_default();
            if !tools.is_empty() {
                println!("    tools:    {} ({})", tools.len(), tools.join(", "));
            }
            let version = text(m, "version");
            if !version.is_empty() {
                println!("    version:  {version}");
            }
        }
        if let Some(page) = book.get(id) {
            let codes: Vec<String> = page
                .advisories
                .iter()
                .map(|e| if e.count > 1 { format!("{} ({}x)", e.code, e.count) } else { e.code.clone() })
                .collect();
            println!("    advised:  {}  (hum doctor says what)", codes.join(", "));
        }
        println!();
    }

    println!("● nestled in humd   ○ entered or exited, not nestled");
    println!("verbs: hum bee enter --hive <hive> [--as <label>]   |   hum bee <id> exit | reenter");
    Ok(())
}
