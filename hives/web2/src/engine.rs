//! One command line in, one answer out: what wrepl is, for the bee and for
//! a person at the REPL alike.
//!
//! `help` lists the services, `<service> help [words]` its commands, and
//! `<service> <command> --help` a command's flags. Anything else is a call.
//! A service only reads unless web2.json turns its writes on, or names the
//! write commands it allows; a command it denies is off either way.

use std::collections::HashMap;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use serde_json::Value;

use crate::auth::{self, Creds, Denied};
use crate::catalogue::{Catalogue, Command, Target, clip, normalize, suggest};
use crate::config::{Auth, Config, Kind, McpServer, Secret, Service, config_path};
use crate::mcp::Pool;
use crate::openapi::{self, AuthHint, Loaded};
use crate::render;

const LIST_ALL_UP_TO: usize = 120;

#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    pub output: String,
    pub is_error: bool,
}

impl Answer {
    fn ok(output: impl Into<String>) -> Answer {
        Answer { output: output.into(), is_error: false }
    }
    fn err(output: impl Into<String>) -> Answer {
        Answer { output: output.into(), is_error: true }
    }
}

enum Out {
    Json(Value),
    Text(String),
}

pub struct Engine {
    pub config: Config,
    pub state: PathBuf,
    pub creds: Arc<Creds>,
    http: reqwest::Client,
    pool: Arc<Pool>,
    specs: tokio::sync::Mutex<HashMap<String, Arc<Loaded>>>,
}

fn is_help(word: &str) -> bool {
    matches!(word, "help" | "--help" | "-h" | "?")
}

impl Engine {
    pub fn new(config: Config, state: PathBuf) -> Engine {
        let idle = std::time::Duration::from_millis(config.idle_ms);
        Engine {
            creds: Creds::at(&state),
            http: reqwest::Client::builder()
                .user_agent(concat!("web2/", env!("CARGO_PKG_VERSION"), " (hum)"))
                .build()
                .unwrap_or_default(),
            pool: Arc::new(Pool::new(idle)),
            specs: tokio::sync::Mutex::new(HashMap::new()),
            config,
            state,
        }
    }

    pub fn pool(&self) -> Arc<Pool> {
        self.pool.clone()
    }

    pub async fn run(&self, line: &str) -> Answer {
        let Some(words) = shlex::split(line) else {
            return Answer::err("the line has a quote that never closes");
        };
        let answer = match words.as_slice() {
            [] => Answer::ok(self.services_help()),
            [only] if is_help(only) => Answer::ok(self.services_help()),
            [h, service, rest @ ..] if is_help(h) => self.help(service, rest).await,
            [service] => self.help(service, &[]).await,
            [service, h, rest @ ..] if is_help(h) => self.help(service, rest).await,
            [service, command, rest @ ..] => {
                if rest.iter().any(|w| w == "--help" || w == "-h") || matches!(rest, [h] if h == "help") {
                    self.help(service, std::slice::from_ref(command)).await
                } else {
                    self.call(service, command, rest).await
                }
            }
        };
        Answer { output: render::trim(answer.output, self.config.max_output), is_error: answer.is_error }
    }

    fn service(&self, name: &str) -> Result<&Service, Answer> {
        self.config.services.get(name).ok_or_else(|| {
            let mut msg = format!("no service is called {name}");
            let close = suggest(name, self.config.services.keys().map(String::as_str));
            if !close.is_empty() {
                msg.push_str(&format!("; did you mean {}?", close.join(" or ")));
            }
            msg.push_str(" `help` lists the services.");
            Answer::err(msg)
        })
    }

    fn signed_in(&self, service: &Service) -> String {
        match &service.auth {
            Some(Auth::Bearer(Secret::Env(v)) | Auth::Header { secret: Secret::Env(v), .. } | Auth::Query { secret: Secret::Env(v), .. } | Auth::Basic { secret: Secret::Env(v), .. }) => {
                format!("token from ${v}")
            }
            _ if self.creds.has(&service.name) => "signed in".into(),
            Some(_) => format!("not signed in: `{} login {}`", auth::cli(), service.name),
            None => match &service.kind {
                Kind::Mcp(McpServer::Stdio { .. }) => "runs here".into(),
                _ => String::new(),
            },
        }
    }

    fn writes(service: &Service) -> String {
        match (service.writes, service.allow.len()) {
            (true, _) => "writes on".into(),
            (false, 0) => "reads only".into(),
            (false, n) => format!("reads, and {n} write{}", if n == 1 { "" } else { "s" }),
        }
    }

    fn description(&self, service: &Service) -> String {
        if let Some(d) = &service.description {
            return d.clone();
        }
        if let Kind::OpenApi { spec, .. } = &service.kind
            && let Some(loaded) = openapi::cached(&self.state, &service.name, spec)
            && let Some(title) = loaded.title
        {
            return title;
        }
        String::new()
    }

    pub fn services_help(&self) -> String {
        if self.config.services.is_empty() {
            return format!(
                "wrepl reaches no services yet. A person adds them to {}: an `openapi` spec or an `mcp` server per service, \
                 or an `mcpServers` block pasted from any harness. `web2 import` copies in the MCP servers the harnesses on \
                 this machine already know.",
                config_path().display()
            );
        }
        let rows: Vec<[String; 5]> = self
            .config
            .services
            .values()
            .map(|s| [s.name.clone(), s.kind_name().into(), clip(&self.description(s), 48), Self::writes(s), self.signed_in(s)])
            .collect();
        let mut out = String::from("wrepl reaches these services. `<service> help` lists a service's commands.\n\n");
        out.push_str(&table(&rows));
        out
    }

    pub async fn catalogue(&self, service: &Service) -> Result<Catalogue, Denied> {
        match &service.kind {
            Kind::OpenApi { .. } => Ok(self.spec(service).await?.catalogue.clone()),
            Kind::Mcp(server) => {
                let applied = self.mcp_auth(service).await?;
                Ok(self.pool.catalogue(service, server, &applied, &self.creds).await?.0)
            }
        }
    }

    async fn spec(&self, service: &Service) -> Result<Arc<Loaded>, Denied> {
        let Kind::OpenApi { spec, base } = &service.kind else {
            return Err(Denied::Failed(format!("{} is not an OpenAPI service", service.name)));
        };
        let mut specs = self.specs.lock().await;
        if let Some(loaded) = specs.get(&service.name) {
            return Ok(loaded.clone());
        }
        let loaded = match openapi::cached(&self.state, &service.name, spec) {
            Some(l) => l,
            None => {
                let l = openapi::fetch(spec, base.as_deref(), &self.config.base_dir, &self.http)
                    .await
                    .map_err(|e| Denied::Failed(format!("{}: {e:#}", service.name)))?;
                openapi::keep(&self.state, &service.name, &l);
                l
            }
        };
        let loaded = Arc::new(loaded);
        specs.insert(service.name.clone(), loaded.clone());
        Ok(loaded)
    }

    pub async fn refresh(&self, service: &Service) -> Result<usize, Denied> {
        if let Kind::OpenApi { .. } = service.kind {
            openapi::forget(&self.state, &service.name);
            self.specs.lock().await.remove(&service.name);
        } else {
            self.pool.relist(&service.name).await;
        }
        Ok(self.catalogue(service).await?.commands.len())
    }

    async fn auth(&self, service: &Service) -> Result<Option<Auth>, Denied> {
        let hint = match &service.kind {
            Kind::OpenApi { .. } => self.spec(service).await?.auth_hint.clone(),
            Kind::Mcp(_) => None,
        };
        Ok(match (&service.auth, hint) {
            (Some(Auth::OAuth2(app)), hint) => Some(Auth::OAuth2(openapi::fill_oauth(app, hint.as_ref()))),
            (Some(auth), _) => Some(auth.clone()),
            (None, Some(AuthHint::Bearer)) => Some(Auth::Bearer(Secret::Stored)),
            (None, Some(AuthHint::Header(name))) => Some(Auth::Header { name, secret: Secret::Stored }),
            (None, Some(AuthHint::Query(name))) => Some(Auth::Query { name, secret: Secret::Stored }),
            (None, Some(AuthHint::Basic)) => None,
            (None, Some(AuthHint::OAuth { .. })) => None,
            (None, None) => None,
        })
    }

    async fn mcp_auth(&self, service: &Service) -> Result<Vec<auth::Applied>, Denied> {
        match &service.auth {
            Some(a) => auth::apply(service, a, &self.creds, &self.http).await,
            None => Ok(Vec::new()),
        }
    }

    async fn help(&self, name: &str, words: &[String]) -> Answer {
        let service = match self.service(name) {
            Ok(s) => s,
            Err(a) => return a,
        };
        let catalogue = match self.catalogue(service).await {
            Ok(c) => c,
            Err(e) => return Answer::err(e.to_string()),
        };
        if let [one] = words
            && let Some(cmd) = catalogue.find(one)
        {
            let mut out = cmd.usage(&service.name);
            if let Err(off) = allowed(service, cmd) {
                out.push('\n');
                out.push_str(&off);
                out.push('\n');
            }
            return Answer::ok(out);
        }
        let mut out = service.name.to_string();
        let description = self.description(service);
        if !description.is_empty() {
            out.push_str(&format!(" — {description}"));
        }
        out.push_str(&format!(" ({}, {} commands, {})\n", service.kind_name(), catalogue.commands.len(), Self::writes(service)));
        out.push_str(&format!("`{} <command> --help` shows a command's flags. A command marked (write, off) is not reached here", service.name));
        if matches!(service.kind, Kind::Mcp(_)) && !service.writes {
            out.push_str("; an MCP tool counts as a write unless it says it only reads");
        }
        out.push_str(".\n");
        let chosen: Vec<&Command> = if words.is_empty() { catalogue.commands.iter().collect() } else { catalogue.matching(words) };
        if chosen.is_empty() {
            out.push_str(&format!("\nNo command matches {:?}.", words.join(" ")));
            return Answer::err(out);
        }
        if chosen.len() > LIST_ALL_UP_TO {
            if words.is_empty() {
                out.push_str(&format!("\nIts commands fall in these groups; `{} help <group>` lists one, `{} help <words>` finds commands by name or summary.\n\n", service.name, service.name));
                let rows: Vec<[String; 5]> = catalogue
                    .groups()
                    .into_iter()
                    .map(|(g, n)| [g, format!("{n} command{}", if n == 1 { "" } else { "s" }), String::new(), String::new(), String::new()])
                    .collect();
                out.push_str(&table(&rows));
            } else {
                out.push_str(&format!("\n{} commands match; add words to narrow it.\n", chosen.len()));
            }
            return Answer::ok(out);
        }
        let mut group: Option<Option<String>> = None;
        let grouped = chosen.iter().any(|c| c.group.is_some());
        let mut rows: Vec<[String; 5]> = Vec::new();
        for cmd in chosen {
            if grouped && group.as_ref() != Some(&cmd.group) {
                if !rows.is_empty() {
                    out.push_str(&table(&rows));
                    rows.clear();
                }
                out.push_str(&format!("\n{}\n", cmd.group.clone().unwrap_or_else(|| "(other)".into())));
                group = Some(cmd.group.clone());
            } else if group.is_none() {
                out.push('\n');
                group = Some(cmd.group.clone());
            }
            let mark = match allowed(service, cmd) {
                Ok(()) if cmd.write => " (write)",
                Ok(()) => "",
                Err(_) if cmd.write => " (write, off)",
                Err(_) => " (off)",
            };
            rows.push([cmd.name.clone(), format!("{}{mark}", clip(&cmd.summary, 90)), String::new(), String::new(), String::new()]);
        }
        out.push_str(&table(&rows));
        Answer::ok(out)
    }

    async fn call(&self, name: &str, command: &str, words: &[String]) -> Answer {
        let service = match self.service(name) {
            Ok(s) => s,
            Err(a) => return a,
        };
        let mut catalogue = match self.catalogue(service).await {
            Ok(c) => c,
            Err(e) => return Answer::err(e.to_string()),
        };
        if catalogue.find(command).is_none() && matches!(service.kind, Kind::Mcp(_)) {
            self.pool.relist(&service.name).await;
            if let Ok(c) = self.catalogue(service).await {
                catalogue = c;
            }
        }
        let Some(cmd) = catalogue.find(command) else {
            let mut msg = format!("{} has no command {command}", service.name);
            let close = catalogue.suggest(command);
            if !close.is_empty() {
                msg.push_str(&format!("; did you mean {}?", close.join(" or ")));
            }
            msg.push_str(&format!(" `{} help` lists its commands.", service.name));
            return Answer::err(msg);
        };
        if let Err(off) = allowed(service, cmd) {
            return Answer::err(off);
        }
        let input = match cmd.build_input(words) {
            Ok(i) => i,
            Err(e) => return Answer::err(format!("{e}. `{} {} --help` shows its flags.", service.name, cmd.name)),
        };
        let timeout = service.timeout(&self.config);
        let out = match tokio::time::timeout(timeout, self.dispatch(service, cmd, input.args)).await {
            Ok(out) => out,
            Err(_) => Err(Answer::err(format!("{} {} gave no answer within {}s", service.name, cmd.name, timeout.as_secs()))),
        };
        let (out, is_error) = match out {
            Ok(pair) => pair,
            Err(answer) => return answer,
        };
        let output = match (&input.jq, out) {
            (Some(filter), Out::Json(v)) if !is_error => match render::jq(filter, &v) {
                Ok(text) => text,
                Err(e) => return Answer::err(e),
            },
            (Some(_), Out::Text(t)) if !is_error => {
                return Answer::err(format!("--jq needs a JSON result, and this one is text:\n{}", clip(&t, 2000)));
            }
            (_, Out::Json(v)) => render::json(&v),
            (_, Out::Text(t)) => t,
        };
        Answer { output, is_error }
    }

    async fn dispatch(&self, service: &Service, cmd: &Command, args: serde_json::Map<String, Value>) -> Result<(Out, bool), Answer> {
        match (&cmd.target, &service.kind) {
            (Target::Http(op), Kind::OpenApi { .. }) => {
                let spec = self.spec(service).await.map_err(|e| Answer::err(e.to_string()))?;
                let applied = match self.auth(service).await.map_err(|e| Answer::err(e.to_string()))? {
                    Some(a) => match auth::apply(service, &a, &self.creds, &self.http).await {
                        Ok(applied) => applied,
                        Err(Denied::NeedsLogin(_)) if service.auth.is_none() => Vec::new(),
                        Err(e) => return Err(Answer::err(e.to_string())),
                    },
                    None => Vec::new(),
                };
                let reply = openapi::call(&self.http, &spec.base, op, &args, &applied, service.timeout(&self.config))
                    .await
                    .map_err(|e| Answer::err(format!("{} {}: {e}", service.name, cmd.name)))?;
                let ok = (200..300).contains(&reply.status);
                let out = match reply.text {
                    Some(t) => Out::Text(t),
                    None => Out::Json(reply.body),
                };
                if ok {
                    return Ok((out, false));
                }
                let body = match out {
                    Out::Json(v) => render::json(&v),
                    Out::Text(t) => t,
                };
                let mut msg = format!("{} {} answered HTTP {}", service.name, cmd.name, reply.status);
                if matches!(reply.status, 401 | 403) && applied.is_empty() {
                    msg.push_str(&format!(" (web2 sent no credentials; `{} login {}` signs it in, if it takes a token)", auth::cli(), service.name));
                }
                if !body.trim().is_empty() {
                    msg.push('\n');
                    msg.push_str(&body);
                }
                Ok((Out::Text(msg), true))
            }
            (Target::Mcp { tool }, Kind::Mcp(server)) => {
                let applied = self.mcp_auth(service).await.map_err(|e| Answer::err(e.to_string()))?;
                let result = self
                    .pool
                    .call(service, server, &applied, &self.creds, tool, args)
                    .await
                    .map_err(|e| Answer::err(e.to_string()))?;
                Ok(mcp_out(&result))
            }
            _ => Err(Answer::err(format!("{} {} does not match its service's kind", service.name, cmd.name))),
        }
    }

    pub async fn login(&self, name: &str, token_from_stdin: bool) -> Result<String> {
        let service = self.config.services.get(name).ok_or_else(|| anyhow!("no service is called {name}; `web2 status` lists them"))?;
        if let Kind::Mcp(server) = &service.kind
            && service.auth.is_none()
        {
            let port = crate::config::DEFAULT_REDIRECT_PORT;
            return crate::mcp::login(service, server, &self.creds, port).await;
        }
        let effective = self.auth(service).await.map_err(|e| anyhow!("{e}"))?;
        match effective {
            None => bail!(
                "{name} names no way to sign in, in its spec or in web2.json; give it an `auth` there (bearer, header, query, basic or oauth)"
            ),
            Some(Auth::OAuth2(app)) => {
                auth::login_oauth(name, &app, &self.creds, &self.http, |url| auth::show_sign_in(name, url)).await?;
                Ok(format!("{name} is signed in"))
            }
            Some(
                Auth::Bearer(Secret::Env(var))
                | Auth::Header { secret: Secret::Env(var), .. }
                | Auth::Query { secret: Secret::Env(var), .. }
                | Auth::Basic { secret: Secret::Env(var), .. },
            ) => Ok(format!("{name} reads its token from ${var} where web2 runs; there is nothing to store")),
            Some(_) => {
                let token = read_token(name, token_from_stdin)?;
                auth::store_token(name, &token, &self.creds)?;
                Ok(format!("{name}'s token is stored"))
            }
        }
    }
}

fn read_token(name: &str, from_stdin: bool) -> Result<String> {
    if from_stdin || !std::io::stdin().is_terminal() {
        let mut text = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut text)?;
        return Ok(text);
    }
    Ok(rpassword::prompt_password(format!("{name}'s token (it is not shown): "))?)
}

pub fn allowed(service: &Service, cmd: &Command) -> Result<(), String> {
    let named = |list: &[String]| {
        list.iter().any(|n| {
            let n = normalize(n);
            n == normalize(&cmd.name) || n == normalize(&cmd.original)
        })
    };
    let (s, c) = (&service.name, &cmd.name);
    if named(&service.deny) {
        return Err(format!("{s} {c} is turned off here (services.{s}.deny in web2.json)"));
    }
    if cmd.write && !service.writes && !named(&service.allow) {
        return Err(format!(
            "{s} {c} writes, and {s} only reads here. A person turns it on in web2.json: \
             services.{s}.allow [\"{c}\"], or services.{s}.writes true for every write"
        ));
    }
    Ok(())
}

fn mcp_out(result: &Value) -> (Out, bool) {
    let is_error = result.get("isError").and_then(Value::as_bool) == Some(true);
    if let Some(structured) = result.get("structuredContent").filter(|v| !v.is_null()) {
        return (Out::Json(structured.clone()), is_error);
    }
    let blocks = result.get("content").and_then(Value::as_array).cloned().unwrap_or_default();
    let texts: Vec<String> = blocks.iter().map(block_text).collect();
    if let [only] = texts.as_slice()
        && blocks[0].get("type").and_then(Value::as_str) == Some("text")
        && let Ok(v @ (Value::Object(_) | Value::Array(_))) = serde_json::from_str::<Value>(only)
    {
        return (Out::Json(v), is_error);
    }
    (Out::Text(texts.join("\n")), is_error)
}

fn block_text(block: &Value) -> String {
    let field = |k: &str| block.get(k).and_then(Value::as_str).unwrap_or("");
    match field("type") {
        "text" => field("text").to_string(),
        kind @ ("image" | "audio") => format!("[{kind}: {}, about {} bytes]", field("mimeType"), field("data").len() * 3 / 4),
        "resource" => {
            let r = block.get("resource").cloned().unwrap_or(Value::Null);
            match r.get("text").and_then(Value::as_str) {
                Some(t) => t.to_string(),
                None => format!("[resource {}]", r.get("uri").and_then(Value::as_str).unwrap_or("")),
            }
        }
        "resource_link" => format!("[link {} {}]", field("uri"), field("name")).trim_end().to_string(),
        other => format!("[{other} content]"),
    }
}

fn table(rows: &[[String; 5]]) -> String {
    let mut widths = [0usize; 5];
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.chars().count());
        }
    }
    let mut out = String::new();
    for row in rows {
        let mut line = String::from("  ");
        for (i, cell) in row.iter().enumerate() {
            if widths[i] == 0 {
                continue;
            }
            line.push_str(cell);
            let pad = widths[i] - cell.chars().count();
            line.push_str(&" ".repeat(pad + 2));
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_json_text_block_is_json_so_jq_can_read_it() {
        let (out, err) = mcp_out(&json!({ "content": [{ "type": "text", "text": "{\"a\":1}" }] }));
        assert!(matches!(out, Out::Json(v) if v["a"] == 1));
        assert!(!err);
        let (out, err) = mcp_out(&json!({ "content": [{ "type": "text", "text": "nope" }, { "type": "image", "mimeType": "image/png", "data": "AAAA" }], "isError": true }));
        assert!(matches!(out, Out::Text(t) if t == "nope\n[image: image/png, about 3 bytes]"));
        assert!(err);
        let (out, _) = mcp_out(&json!({ "content": [], "structuredContent": { "n": 2 } }));
        assert!(matches!(out, Out::Json(v) if v["n"] == 2));
    }

    #[test]
    fn the_table_lines_its_columns_up() {
        let t = table(&[
            ["a".into(), "bb".into(), String::new(), String::new(), String::new()],
            ["ccc".into(), "d".into(), String::new(), String::new(), String::new()],
        ]);
        assert_eq!(t, "  a    bb\n  ccc  d\n");
    }
}
