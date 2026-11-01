//! `web2 import`: the MCP servers the harnesses on this machine already
//! know, copied into web2.json's `mcpServers` once, so each is configured
//! and signed in to in one place.
//!
//! It reads Claude Code (`~/.claude.json`, and `.mcp.json` where it runs),
//! Claude Desktop, Cursor, VS Code and Codex. A name web2 already has is
//! left alone, and so is a server that is hum's own. A server is added only
//! as web2 reads it wherever it runs: one the loader refuses, or one that
//! fills a `${…}` from the environment, which the bee may not share, is
//! left out with the reason, so an import never breaks web2.json.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};

pub struct Found {
    pub name: String,
    pub from: String,
    pub server: Value,
}

fn home() -> PathBuf {
    hum_paths::home()
}

pub fn sources() -> Vec<(String, PathBuf)> {
    let h = home();
    let mut list = vec![
        ("Claude Code".to_string(), h.join(".claude.json")),
        ("Claude Code (here)".to_string(), PathBuf::from(".mcp.json")),
        ("Cursor".to_string(), h.join(".cursor/mcp.json")),
        ("Codex".to_string(), h.join(".codex/config.toml")),
    ];
    if cfg!(target_os = "macos") {
        list.push(("Claude Desktop".into(), h.join("Library/Application Support/Claude/claude_desktop_config.json")));
        list.push(("VS Code".into(), h.join("Library/Application Support/Code/User/mcp.json")));
    } else {
        list.push(("Claude Desktop".into(), h.join(".config/Claude/claude_desktop_config.json")));
        list.push(("VS Code".into(), h.join(".config/Code/User/mcp.json")));
    }
    list
}

fn is_hum(name: &str, server: &Value) -> bool {
    let url = server.get("url").and_then(Value::as_str).unwrap_or("");
    name == "hum" || ((url.contains("127.0.0.1") || url.contains("localhost")) && url.contains("/s/"))
}

pub fn read_source(label: &str, path: &Path) -> Result<Vec<Found>> {
    let Ok(text) = std::fs::read_to_string(path) else { return Ok(Vec::new()) };
    let servers: Map<String, Value> = if path.extension().is_some_and(|e| e == "toml") {
        let doc: toml::Value = toml::from_str(&text).with_context(|| format!("read {}", path.display()))?;
        let as_json = serde_json::to_value(doc)?;
        as_json.get("mcp_servers").and_then(Value::as_object).cloned().unwrap_or_default()
    } else {
        let doc: Value = serde_json::from_str(&text).with_context(|| format!("read {}", path.display()))?;
        doc.get("mcpServers").or_else(|| doc.get("servers")).and_then(Value::as_object).cloned().unwrap_or_default()
    };
    Ok(servers
        .into_iter()
        .filter(|(name, server)| !is_hum(name, server))
        .map(|(name, server)| Found { name, from: label.to_string(), server: tidy(server) })
        .collect())
}

fn tidy(server: Value) -> Value {
    let Value::Object(obj) = server else { return server };
    let keep = ["command", "args", "env", "cwd", "url", "type", "headers"];
    let mut out: Map<String, Value> = obj.into_iter().filter(|(k, _)| keep.contains(&k.as_str())).collect();
    if let Some(Value::String(t)) = out.get("type")
        && t == "streamable-http"
    {
        out.insert("type".into(), json!("http"));
    }
    Value::Object(out)
}

pub struct Plan {
    pub added: Vec<Found>,
    pub skipped: Vec<(String, String)>,
}

fn placeholders(v: &Value, out: &mut BTreeSet<String>) {
    match v {
        Value::String(s) => out.extend(s.match_indices("${").filter_map(|(i, _)| s[i..].find('}').map(|j| s[i..=i + j].to_string()))),
        Value::Object(o) => o.values().for_each(|v| placeholders(v, out)),
        _ => {}
    }
}

fn unreadable(name: &str, server: &Value) -> Option<String> {
    let mut filled = BTreeSet::new();
    for key in ["url", "env", "headers"] {
        if let Some(v) = server.get(key) {
            placeholders(v, &mut filled);
        }
    }
    if !filled.is_empty() {
        let filled: Vec<String> = filled.into_iter().collect();
        return Some(format!("it reads {} from the environment web2 runs in, which an import cannot check; add it by hand", filled.join(", ")));
    }
    let one = json!({ "mcpServers": Map::from_iter([(name.to_string(), server.clone())]) });
    crate::config::parse(&one.to_string(), PathBuf::new()).err().map(|e| e.root_cause().to_string())
}

pub fn plan(existing: &Value, found: Vec<Found>) -> Plan {
    let mut taken = BTreeSet::new();
    for key in ["services", "mcpServers"] {
        if let Some(obj) = existing.get(key).and_then(Value::as_object) {
            taken.extend(obj.keys().cloned());
        }
    }
    let mut added = Vec::new();
    let mut skipped = Vec::new();
    for f in found {
        if taken.contains(&f.name) {
            skipped.push((f.name, format!("web2 already has it (from {})", f.from)));
        } else if let Some(why) = unreadable(&f.name, &f.server) {
            skipped.push((f.name, format!("{why} (from {})", f.from)));
        } else {
            taken.insert(f.name.clone());
            added.push(f);
        }
    }
    Plan { added, skipped }
}

pub fn apply(path: &Path, plan: &Plan) -> Result<()> {
    crate::config::edit_at(path, |doc| {
        let Value::Object(servers) = doc.entry("mcpServers").or_insert_with(|| json!({})) else {
            bail!("mcpServers in web2.json is not an object");
        };
        for f in &plan.added {
            servers.entry(f.name.clone()).or_insert_with(|| f.server.clone());
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_harness_shape_is_read_and_hum_is_left_out() {
        let dir = tempfile::tempdir().unwrap();
        let claude = dir.path().join("claude.json");
        std::fs::write(&claude, r#"{ "mcpServers": {
            "sentry": { "type": "http", "url": "https://mcp.sentry.dev/mcp", "oauth": { "x": 1 } },
            "hum": { "type": "http", "url": "http://127.0.0.1:4111/s/abc" } }, "other": 1 }"#).unwrap();
        let vscode = dir.path().join("mcp.json");
        std::fs::write(&vscode, r#"{ "servers": { "fs": { "type": "stdio", "command": "npx", "args": ["-y", "fs"] } } }"#).unwrap();
        let codex = dir.path().join("config.toml");
        std::fs::write(&codex, "[mcp_servers.linear]\ncommand = \"npx\"\nargs = [\"-y\", \"linear\"]\n").unwrap();
        let a = read_source("Claude Code", &claude).unwrap();
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].server, json!({ "type": "http", "url": "https://mcp.sentry.dev/mcp" }));
        assert_eq!(read_source("VS Code", &vscode).unwrap()[0].name, "fs");
        assert_eq!(read_source("Codex", &codex).unwrap()[0].server["command"], "npx");
        assert!(read_source("none", &dir.path().join("missing.json")).unwrap().is_empty());
    }

    #[test]
    fn a_name_web2_has_is_kept_and_the_rest_are_written_privately() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("web2.json");
        let existing = json!({
            "mcpServers": { "sentry": { "url": "https://mcp.sentry.dev/mcp" } },
            "services": { "sentry": { "writes": true } },
            "maxOutput": 1000
        });
        let found = vec![
            Found { name: "sentry".into(), from: "Cursor".into(), server: json!({ "url": "https://a" }) },
            Found { name: "fs".into(), from: "Cursor".into(), server: json!({ "command": "npx" }) },
            Found { name: "fs".into(), from: "Codex".into(), server: json!({ "command": "other" }) },
        ];
        let p = plan(&existing, found);
        assert_eq!(p.added.len(), 1);
        assert_eq!(p.skipped.len(), 2);
        std::fs::write(&path, existing.to_string()).unwrap();
        apply(&path, &p).unwrap();
        let written: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(written["mcpServers"]["fs"]["command"], "npx");
        assert_eq!(written["maxOutput"], 1000);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let parsed = crate::config::load_from(&path).unwrap();
        assert!(parsed.services.contains_key("fs"));
    }

    #[test]
    fn a_server_web2_cannot_read_everywhere_is_left_out_and_said_why() {
        let found = |name: &str, server: Value| Found { name: name.into(), from: "VS Code".into(), server };
        let p = plan(
            &json!({}),
            vec![
                found("old", json!({ "type": "sse", "url": "https://x/sse" })),
                found("asks", json!({ "command": "npx", "env": { "TOKEN": "${input:token}" } })),
                found("home", json!({ "type": "http", "url": "https://x/mcp", "headers": { "X-Home": "${HOME}/${HOME}" } })),
                found("bare", json!({ "args": ["x"] })),
                found("dotted.name", json!({ "command": "x" })),
                found("asks", json!({ "command": "npx", "args": ["${literal}"] })),
            ],
        );
        let why: std::collections::HashMap<&str, &str> = p.skipped.iter().map(|(n, w)| (n.as_str(), w.as_str())).collect();
        assert!(why["old"].starts_with("legacy SSE"), "{}", why["old"]);
        assert!(why["asks"].contains("${input:token}") && why["asks"].ends_with("(from VS Code)"), "{}", why["asks"]);
        assert!(why["home"].contains("reads ${HOME} from"), "{}", why["home"]);
        assert!(why["bare"].contains("needs a `command`"), "{}", why["bare"]);
        assert!(why["dotted.name"].contains("letters, digits"), "{}", why["dotted.name"]);
        assert_eq!(p.added.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["asks"]);
    }

    #[test]
    fn an_import_into_a_broken_config_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("web2.json");
        let broken = r#"{ "services": { "x": { "openapi": "a", "colour": "red" } } }"#;
        std::fs::write(&path, broken).unwrap();
        let p = plan(&json!({}), vec![Found { name: "fs".into(), from: "Cursor".into(), server: json!({ "command": "npx" }) }]);
        assert!(apply(&path, &p).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), broken);
    }
}
