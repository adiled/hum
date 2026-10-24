//! Bees — the participants a humd hears from.
//!
//! hum starts no bee. A hive enters one — mints its key and signs it in
//! under the hive — and whatever the hive runs as that bee signs its
//! hello with the key. Exiting signs it out; humd refuses its next hello.

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

pub fn enter(hive: &str) -> Result<()> {
    if !hum_paths::hive_installed(hive) {
        bail!("{hive} is not installed on this humd — `hum hive <ref> install` first");
    }
    let key = hum_identity::mint_bee_key()?;
    hum_identity::enter(hive, &key)?;
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
        "enter" => bail!("a bee is entered under a hive, not by id: hum bee enter --hive <hive>"),
        other => bail!("unknown verb '{other}' (exit | reenter)"),
    }
    Ok(())
}

pub fn list() -> Result<()> {
    let entered = hum_identity::entered();
    let nestled = nestled();
    if entered.is_empty() && nestled.is_empty() {
        println!("no bee has entered this humd.");
        println!("a hive enters one: hum bee enter --hive <hive>");
        return Ok(());
    }

    let nest_of = |id: &str| {
        let hid = Hid::from_hex(id).ok()?;
        nestled
            .iter()
            .find(|m| Hid::from_hex(&text(m, "hid")).is_ok_and(|h| h.same_key(&hid)))
    };

    for (id, entry) in &entered {
        let state = match (entry.exited, nest_of(id)) {
            (true, _) => "exited",
            (false, Some(_)) => "nestled",
            (false, None) => "entered, not nestled",
        };
        let mark = if state == "nestled" { "●" } else { "○" };
        println!("{mark} {id}  —  hive {}, {state}", entry.hive);
        if let Some(m) = nest_of(id) {
            let mut roles = texts(m, "roles");
            if roles.is_empty() {
                roles = texts(m, "bee");
            }
            if !roles.is_empty() {
                println!("    roles:    {}", roles.join("+"));
            }
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
        println!();
    }

    println!("● nestled in humd   ○ entered or exited, not nestled");
    println!("verbs: hum bee enter --hive <hive>   |   hum bee <id> exit | reenter");
    Ok(())
}
