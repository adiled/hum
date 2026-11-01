//! web2's config: the services it reaches, read from the hive's own file.
//!
//! The file is `hives/web2.json` under hum's config directory, or
//! `$WEB2_CONFIG`. `services` names each one: an `openapi` spec, or an
//! `mcp` server. An `mcpServers` block, pasted verbatim from any harness's
//! config, makes each of its servers a service too, and an entry in
//! `services` with no kind of its own sets that server's policy by name.
//!
//! `${VAR}` in a base URL, a header or an env value reads the environment
//! web2 runs in.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use serde_json::Value;

pub const HIVE: &str = "web2";
pub const DEFAULT_MAX_OUTPUT: usize = 32 * 1024;
pub const DEFAULT_TIMEOUT_MS: u64 = 120_000;
pub const DEFAULT_IDLE_MS: u64 = 600_000;
pub const DEFAULT_REDIRECT_PORT: u16 = 8976;

pub fn config_path() -> PathBuf {
    std::env::var_os("WEB2_CONFIG").map(PathBuf::from).unwrap_or_else(|| hum_paths::hive_config(HIVE))
}

pub fn state_dir() -> PathBuf {
    std::env::var_os("WEB2_STATE").map(PathBuf::from).unwrap_or_else(|| hum_paths::hive_state_dir(HIVE))
}

#[derive(Debug, Clone)]
pub struct Config {
    pub services: BTreeMap<String, Service>,
    pub max_output: usize,
    pub timeout_ms: u64,
    pub idle_ms: u64,
    pub redirect_port: u16,
    pub serve_peers: bool,
    pub base_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub struct Service {
    pub name: String,
    pub kind: Kind,
    pub description: Option<String>,
    pub writes: bool,
    pub allow: Vec<String>,
    pub deny: Vec<String>,
    pub auth: Option<Auth>,
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone)]
pub enum Kind {
    OpenApi { spec: String, base: Option<String> },
    Mcp(McpServer),
}

#[derive(Debug, Clone)]
pub enum McpServer {
    Stdio { command: String, args: Vec<String>, env: BTreeMap<String, String>, cwd: Option<String> },
    Http { url: String, headers: BTreeMap<String, String> },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Secret {
    Stored,
    Env(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Auth {
    Bearer(Secret),
    Header { name: String, secret: Secret },
    Query { name: String, secret: Secret },
    Basic { username: String, secret: Secret },
    OAuth2(OAuthApp),
}

#[derive(Debug, Clone, PartialEq)]
pub struct OAuthApp {
    pub client_id: String,
    pub client_secret: Option<String>,
    pub authorize_url: Option<String>,
    pub token_url: Option<String>,
    pub scopes: Vec<String>,
    pub redirect_port: u16,
}

impl Service {
    pub fn kind_name(&self) -> &'static str {
        match self.kind {
            Kind::OpenApi { .. } => "openapi",
            Kind::Mcp(McpServer::Stdio { .. }) => "mcp",
            Kind::Mcp(McpServer::Http { .. }) => "mcp",
        }
    }

    pub fn timeout(&self, config: &Config) -> std::time::Duration {
        std::time::Duration::from_millis(self.timeout_ms.unwrap_or(config.timeout_ms))
    }
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct RawConfig {
    #[serde(default)]
    services: BTreeMap<String, Value>,
    #[serde(default)]
    mcp_servers: BTreeMap<String, Value>,
    max_output: Option<usize>,
    timeout_ms: Option<u64>,
    idle_ms: Option<u64>,
    redirect_port: Option<u16>,
    #[serde(default)]
    serve_peers: bool,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawService {
    openapi: Option<String>,
    base: Option<String>,
    mcp: Option<RawMcp>,
    description: Option<String>,
    #[serde(default)]
    writes: bool,
    #[serde(default)]
    allow: Vec<String>,
    #[serde(default)]
    deny: Vec<String>,
    auth: Option<Value>,
    timeout_ms: Option<u64>,
}

#[derive(Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct RawMcp {
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    cwd: Option<String>,
    url: Option<String>,
    #[serde(rename = "type")]
    transport: Option<String>,
    #[serde(default)]
    headers: BTreeMap<String, String>,
}

pub fn load() -> Result<Config> {
    load_from(&config_path())
}

pub fn load_from(path: &Path) -> Result<Config> {
    let base_dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let raw = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Config { services: BTreeMap::new(), ..defaults(base_dir) });
        }
        Err(e) => return Err(e).with_context(|| format!("read {}", path.display())),
    };
    parse(&raw, base_dir).with_context(|| format!("in {}", path.display()))
}

fn defaults(base_dir: PathBuf) -> Config {
    Config {
        services: BTreeMap::new(),
        max_output: DEFAULT_MAX_OUTPUT,
        timeout_ms: DEFAULT_TIMEOUT_MS,
        idle_ms: DEFAULT_IDLE_MS,
        redirect_port: DEFAULT_REDIRECT_PORT,
        serve_peers: false,
        base_dir,
    }
}

pub fn parse(raw: &str, base_dir: PathBuf) -> Result<Config> {
    let raw: RawConfig = serde_json::from_str(raw).context("web2.json is not the shape web2 reads")?;
    let mcp_servers: BTreeMap<String, RawMcp> = raw
        .mcp_servers
        .iter()
        .map(|(name, v)| Ok((name.clone(), serde_json::from_value(v.clone()).with_context(|| format!("mcpServers.{name}"))?)))
        .collect::<Result<_>>()?;
    let mut config = Config {
        max_output: raw.max_output.unwrap_or(DEFAULT_MAX_OUTPUT),
        timeout_ms: raw.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS),
        idle_ms: raw.idle_ms.unwrap_or(DEFAULT_IDLE_MS),
        redirect_port: raw.redirect_port.unwrap_or(DEFAULT_REDIRECT_PORT),
        serve_peers: raw.serve_peers,
        ..defaults(base_dir)
    };
    for (name, server) in &mcp_servers {
        if raw.services.contains_key(name) {
            continue;
        }
        let service = service(name, RawService { mcp: Some(server.clone()), ..RawService::default() }, &mcp_servers, config.redirect_port)
            .with_context(|| format!("mcpServers.{name}"))?;
        config.services.insert(name.clone(), service);
    }
    for (name, entry) in raw.services {
        let entry: RawService = serde_json::from_value(entry).with_context(|| format!("services.{name}"))?;
        let service = service(&name, entry, &mcp_servers, config.redirect_port).with_context(|| format!("services.{name}"))?;
        config.services.insert(name, service);
    }
    Ok(config)
}

fn service(name: &str, raw: RawService, mcp_servers: &BTreeMap<String, RawMcp>, port: u16) -> Result<Service> {
    if !is_service_name(name) {
        bail!("a service name is letters, digits, `-` and `_`, and does not start with `-`");
    }
    let kind = match (raw.openapi, raw.mcp, mcp_servers.get(name)) {
        (Some(_), Some(_), _) => bail!("a service is `openapi` or `mcp`, not both"),
        (Some(spec), None, _) => Kind::OpenApi { spec, base: raw.base.as_deref().map(interpolate).transpose()? },
        (None, Some(mcp), _) => Kind::Mcp(mcp_server(&mcp)?),
        (None, None, Some(mcp)) => Kind::Mcp(mcp_server(mcp)?),
        (None, None, None) => bail!("needs `openapi` or `mcp`, or an entry of the same name in `mcpServers`"),
    };
    if raw.auth.is_some() && matches!(kind, Kind::Mcp(McpServer::Stdio { .. })) {
        bail!("a stdio MCP server takes no `auth`: it reads its credentials from its environment, so give them in its `env` (`${{VAR}}` reads one from web2's)");
    }
    Ok(Service {
        name: name.to_string(),
        kind,
        description: raw.description,
        writes: raw.writes,
        allow: raw.allow,
        deny: raw.deny,
        auth: raw.auth.as_ref().map(|a| auth(a, port)).transpose()?,
        timeout_ms: raw.timeout_ms,
    })
}

pub fn glob(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let n: Vec<char> = name.chars().collect();
    let (mut pi, mut ni, mut star, mut mark) = (0, 0, None, 0);
    while ni < n.len() {
        if pi < p.len() && p[pi] != '*' && p[pi] == n[ni] {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ni;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

pub fn is_service_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn mcp_server(raw: &RawMcp) -> Result<McpServer> {
    let transport = raw.transport.as_deref().unwrap_or(if raw.url.is_some() { "http" } else { "stdio" });
    match (transport, &raw.command, &raw.url) {
        ("stdio", Some(command), _) => Ok(McpServer::Stdio {
            command: command.clone(),
            args: raw.args.clone(),
            env: raw.env.iter().map(|(k, v)| Ok((k.clone(), interpolate(v)?))).collect::<Result<_>>()?,
            cwd: raw.cwd.clone(),
        }),
        ("stdio", None, _) => bail!("a stdio MCP server needs a `command`"),
        ("http" | "streamable-http" | "streamableHttp", _, Some(url)) => Ok(McpServer::Http {
            url: interpolate(url)?,
            headers: raw.headers.iter().map(|(k, v)| Ok((k.clone(), interpolate(v)?))).collect::<Result<_>>()?,
        }),
        ("sse", _, _) => bail!("legacy SSE MCP servers are not reached; most now serve streamable HTTP, `type: http`"),
        (other, _, None) if other != "stdio" => bail!("an MCP server of type `{other}` needs a `url`"),
        (other, _, _) => bail!("MCP transport `{other}` is not one web2 speaks (stdio, http)"),
    }
}

fn auth(raw: &Value, port: u16) -> Result<Auth> {
    let secret = |v: &Value| match v.get("env").and_then(Value::as_str) {
        Some(var) => Secret::Env(var.to_string()),
        None => Secret::Stored,
    };
    if let Some(kind) = raw.as_str() {
        return match kind {
            "bearer" => Ok(Auth::Bearer(Secret::Stored)),
            other => bail!("auth `{other}` needs its details: {{\"{other}\": …}}"),
        };
    }
    let obj = raw.as_object().ok_or_else(|| anyhow!("auth is a string or an object"))?;
    if let Some(b) = obj.get("bearer") {
        return Ok(Auth::Bearer(if b.is_object() { secret(b) } else { secret(raw) }));
    }
    if let Some(name) = obj.get("header").and_then(Value::as_str) {
        return Ok(Auth::Header { name: name.to_string(), secret: secret(raw) });
    }
    if let Some(name) = obj.get("query").and_then(Value::as_str) {
        return Ok(Auth::Query { name: name.to_string(), secret: secret(raw) });
    }
    if let Some(username) = obj.get("basic").and_then(Value::as_str) {
        return Ok(Auth::Basic { username: interpolate(username)?, secret: secret(raw) });
    }
    if let Some(app) = obj.get("oauth") {
        let text = |k: &str| app.get(k).and_then(Value::as_str).map(interpolate).transpose();
        return Ok(Auth::OAuth2(OAuthApp {
            client_id: text("clientId")?.ok_or_else(|| anyhow!("oauth needs a `clientId`"))?,
            client_secret: text("clientSecret")?,
            authorize_url: text("authorizeUrl")?,
            token_url: text("tokenUrl")?,
            scopes: app
                .get("scopes")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
                .unwrap_or_default(),
            redirect_port: app
                .get("redirectPort")
                .and_then(Value::as_u64)
                .and_then(|p| u16::try_from(p).ok())
                .unwrap_or(port),
        }));
    }
    bail!("auth is one of bearer, header, query, basic or oauth")
}

pub fn edit(change: impl FnOnce(&mut serde_json::Map<String, Value>) -> Result<()>) -> Result<()> {
    edit_at(&config_path(), change)
}

pub fn edit_at(path: &Path, change: impl FnOnce(&mut serde_json::Map<String, Value>) -> Result<()>) -> Result<()> {
    let mut doc = match std::fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str::<Value>(&text).with_context(|| format!("read {}", path.display()))? {
            Value::Object(obj) => obj,
            _ => bail!("{} is not a JSON object", path.display()),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => serde_json::Map::new(),
        Err(e) => return Err(e).with_context(|| format!("read {}", path.display())),
    };
    change(&mut doc)?;
    let text = serde_json::to_string_pretty(&Value::Object(doc))? + "\n";
    parse(&text, path.parent().map(Path::to_path_buf).unwrap_or_default()).context("the change would break web2.json, so it was not made")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let staged = path.with_extension(format!("json.{}", std::process::id()));
    crate::auth::write_private(&staged, text.as_bytes())?;
    std::fs::rename(&staged, path).with_context(|| format!("place {}", path.display()))
}

pub fn add_to_list(doc: &mut serde_json::Map<String, Value>, service: &str, key: &str, names: &[String]) -> Vec<String> {
    let services = doc.entry("services").or_insert_with(|| serde_json::json!({}));
    let Value::Object(services) = services else { return Vec::new() };
    let entry = services.entry(service).or_insert_with(|| serde_json::json!({}));
    let Value::Object(entry) = entry else { return Vec::new() };
    let list = entry.entry(key).or_insert_with(|| serde_json::json!([]));
    let Value::Array(items) = list else { return Vec::new() };
    let mut added = Vec::new();
    for name in names {
        if !items.iter().any(|i| i.as_str() == Some(name)) {
            items.push(Value::String(name.clone()));
            added.push(name.clone());
        }
    }
    added
}

pub fn remove_from_list(doc: &mut serde_json::Map<String, Value>, service: &str, key: &str, names: &[String]) {
    if let Some(Value::Array(items)) = doc.get_mut("services").and_then(|s| s.get_mut(service)).and_then(|e| e.get_mut(key)) {
        items.retain(|i| !i.as_str().is_some_and(|s| names.iter().any(|n| n == s)));
    }
}

pub fn interpolate(text: &str) -> Result<String> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let end = after.find('}').ok_or_else(|| anyhow!("`${{` with no closing `}}` in {text:?}"))?;
        let var = &after[..end];
        let value = std::env::var(var).map_err(|_| anyhow!("${{{var}}} is not set in web2's environment"))?;
        out.push_str(&value);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(raw: &str) -> Config {
        parse(raw, PathBuf::from("/cfg")).unwrap()
    }

    #[test]
    fn an_mcp_servers_block_pasted_verbatim_becomes_services() {
        let c = parsed(
            r#"{ "mcpServers": {
                "everything": { "command": "npx", "args": ["-y", "server-everything"] },
                "sentry": { "type": "http", "url": "https://mcp.sentry.dev/mcp" } } }"#,
        );
        assert!(matches!(&c.services["everything"].kind, Kind::Mcp(McpServer::Stdio { command, .. }) if command == "npx"));
        assert!(matches!(&c.services["sentry"].kind, Kind::Mcp(McpServer::Http { url, .. }) if url.ends_with("/mcp")));
        assert!(!c.services["sentry"].writes);
    }

    #[test]
    fn a_service_with_no_kind_sets_the_policy_of_the_server_it_names() {
        let c = parsed(
            r#"{ "mcpServers": { "sentry": { "url": "https://mcp.sentry.dev/mcp" } },
                 "services": { "sentry": { "writes": true, "deny": ["delete-project"] } } }"#,
        );
        let s = &c.services["sentry"];
        assert!(s.writes);
        assert_eq!(s.deny, ["delete-project"]);
        assert!(matches!(s.kind, Kind::Mcp(McpServer::Http { .. })));
    }

    #[test]
    fn auth_takes_each_of_its_shapes() {
        let c = parsed(
            r#"{ "services": {
                "a": { "openapi": "a.json", "auth": "bearer" },
                "b": { "openapi": "b.json", "auth": { "bearer": { "env": "B_TOKEN" } } },
                "c": { "openapi": "c.json", "auth": { "header": "X-Api-Key" } },
                "d": { "openapi": "d.json", "auth": { "query": "api_key", "env": "D_KEY" } },
                "e": { "openapi": "e.json", "auth": { "oauth": { "clientId": "id", "scopes": ["repo"] } } } } }"#,
        );
        assert_eq!(c.services["a"].auth, Some(Auth::Bearer(Secret::Stored)));
        assert_eq!(c.services["b"].auth, Some(Auth::Bearer(Secret::Env("B_TOKEN".into()))));
        assert_eq!(c.services["c"].auth, Some(Auth::Header { name: "X-Api-Key".into(), secret: Secret::Stored }));
        assert_eq!(c.services["d"].auth, Some(Auth::Query { name: "api_key".into(), secret: Secret::Env("D_KEY".into()) }));
        let Some(Auth::OAuth2(app)) = &c.services["e"].auth else { panic!("oauth") };
        assert_eq!((app.client_id.as_str(), app.redirect_port), ("id", DEFAULT_REDIRECT_PORT));
    }

    #[test]
    fn a_service_that_is_neither_kind_or_both_is_refused() {
        assert!(parse(r#"{ "services": { "x": {} } }"#, PathBuf::new()).is_err());
        assert!(parse(r#"{ "services": { "x": { "openapi": "a", "mcp": { "command": "y" } } } }"#, PathBuf::new()).is_err());
        assert!(parse(r#"{ "services": { "x": { "openapi": "a", "colour": "red" } } }"#, PathBuf::new()).is_err());
        assert!(parse(r#"{ "mcpServers": { "old": { "type": "sse", "url": "http://x" } } }"#, PathBuf::new()).is_err());
    }

    #[test]
    fn a_stdio_server_is_refused_an_auth_and_pointed_at_its_env() {
        for raw in [
            r#"{ "services": { "x": { "mcp": { "command": "y" }, "auth": "bearer" } } }"#,
            r#"{ "mcpServers": { "x": { "command": "y" } }, "services": { "x": { "auth": { "bearer": { "env": "T" } } } } }"#,
        ] {
            let err = format!("{:#}", parse(raw, PathBuf::new()).unwrap_err());
            assert!(err.contains("services.x") && err.contains("takes no `auth`") && err.contains("`env`"), "{err}");
        }
        let c = parsed(r#"{ "services": { "x": { "mcp": { "url": "https://x/mcp" }, "auth": "bearer" } } }"#);
        assert_eq!(c.services["x"].auth, Some(Auth::Bearer(Secret::Stored)));
    }

    #[test]
    fn interpolation_reads_the_environment_and_names_what_is_missing() {
        unsafe { std::env::set_var("WEB2_TEST_HOST", "api.example.com") };
        assert_eq!(interpolate("https://${WEB2_TEST_HOST}/v1").unwrap(), "https://api.example.com/v1");
        let err = interpolate("${WEB2_TEST_SURELY_UNSET}").unwrap_err().to_string();
        assert!(err.contains("WEB2_TEST_SURELY_UNSET"), "{err}");
    }

    #[test]
    fn a_glob_matches_with_stars_and_nothing_else() {
        assert!(glob("read*", "readwikistructure"));
        assert!(glob("*wiki*", "askwikiquestion"));
        assert!(glob("*", "anything"));
        assert!(glob("issues/create", "issues/create"));
        assert!(glob("repos/*", "repos/delete-2.0"));
        assert!(glob("*", ""));
        assert!(!glob("read*", "askwikiquestion"));
        assert!(!glob("get", "getx"));
    }

    #[test]
    fn the_redirect_port_is_shared_unless_an_app_says_its_own() {
        let c = parsed(r#"{ "redirectPort": 9100, "services": {
            "a": { "openapi": "a", "auth": { "oauth": { "clientId": "x" } } },
            "b": { "openapi": "b", "auth": { "oauth": { "clientId": "y", "redirectPort": 9200 } } } } }"#);
        assert_eq!(c.redirect_port, 9100);
        let port = |n: &str| match &c.services[n].auth { Some(Auth::OAuth2(app)) => app.redirect_port, _ => 0 };
        assert_eq!((port("a"), port("b")), (9100, 9200));
    }

    #[test]
    fn no_file_is_no_services() {
        let c = load_from(Path::new("/no/such/web2.json")).unwrap();
        assert!(c.services.is_empty());
    }
}
