//! An OpenAPI 3 spec, read as commands, and each command made as an HTTP call.
//!
//! Every operation is a command named by its `operationId` (kebab-cased),
//! or by its method and path when it has none, grouped by its first tag.
//! Its path, query and header parameters are flags, and so are the
//! properties of a JSON request body; a body that is not an object is the
//! one flag `--body`. Local `$ref`s are followed, a few levels deep.
//!
//! A spec is read from a file, relative to web2.json's directory, or from
//! a URL, in JSON or YAML, and the commands made from it are kept in the
//! state directory for a day, or until `web2 refresh`.
//!
//! An operation that only reads (GET, HEAD, OPTIONS) is a read; any other
//! method is a write.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::auth::Applied;
use crate::catalogue::{Body, Catalogue, Command, HttpOp, HttpParam, ParamAt, Target, clip, first_line, kebab};
use crate::config::OAuthApp;

const REF_DEPTH: usize = 3;
const CACHE_SECS: u64 = 24 * 60 * 60;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AuthHint {
    Bearer,
    Basic,
    Header(String),
    Query(String),
    OAuth { authorize_url: Option<String>, token_url: Option<String>, scopes: Vec<String> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Loaded {
    pub source: String,
    pub base: String,
    pub title: Option<String>,
    pub auth_hint: Option<AuthHint>,
    pub catalogue: Catalogue,
    pub fetched_at: u64,
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn cache_path(state: &Path, service: &str) -> std::path::PathBuf {
    state.join("catalogue").join(format!("{service}.json"))
}

pub fn cached(state: &Path, service: &str, source: &str) -> Option<Loaded> {
    let text = std::fs::read_to_string(cache_path(state, service)).ok()?;
    let loaded: Loaded = serde_json::from_str(&text).ok()?;
    (loaded.source == source && now().saturating_sub(loaded.fetched_at) < CACHE_SECS).then_some(loaded)
}

pub fn forget(state: &Path, service: &str) {
    let _ = std::fs::remove_file(cache_path(state, service));
}

pub fn keep(state: &Path, service: &str, loaded: &Loaded) {
    let path = cache_path(state, service);
    if let Some(dir) = path.parent()
        && std::fs::create_dir_all(dir).is_ok()
        && let Ok(text) = serde_json::to_string(loaded)
    {
        let _ = std::fs::write(path, text);
    }
}

pub async fn fetch(source: &str, base_override: Option<&str>, base_dir: &Path, http: &reqwest::Client) -> Result<Loaded> {
    let (text, spec_url) = if source.starts_with("http://") || source.starts_with("https://") {
        let resp = http.get(source).send().await.with_context(|| format!("fetch {source}"))?;
        let status = resp.status();
        if !status.is_success() {
            bail!("fetching {source} answered HTTP {status}");
        }
        (resp.text().await?, Some(source.to_string()))
    } else {
        let path = if Path::new(source).is_absolute() { source.into() } else { base_dir.join(source) };
        (std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?, None)
    };
    let spec: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => serde_yaml_ng::from_str(&text).with_context(|| format!("{source} is neither JSON nor YAML"))?,
    };
    read(&spec, source, base_override, spec_url.as_deref())
}

pub fn read(spec: &Value, source: &str, base_override: Option<&str>, spec_url: Option<&str>) -> Result<Loaded> {
    if spec.get("swagger").is_some() {
        bail!("{source} is Swagger 2.0; web2 reads OpenAPI 3, so convert it first");
    }
    let version = spec.get("openapi").and_then(Value::as_str).unwrap_or("");
    if !version.starts_with('3') {
        bail!("{source} is not an OpenAPI 3 spec (no `openapi: 3.x`)");
    }
    let base = match base_override {
        Some(b) => b.to_string(),
        None => server_url(spec, spec_url).ok_or_else(|| anyhow!("{source} names no server; set `base` in web2.json"))?,
    };
    let commands = commands(spec)?;
    Ok(Loaded {
        source: source.to_string(),
        base: base.trim_end_matches('/').to_string(),
        title: spec.pointer("/info/title").and_then(Value::as_str).map(str::to_string),
        auth_hint: auth_hint(spec),
        catalogue: Catalogue::new(commands),
        fetched_at: now(),
    })
}

fn server_url(spec: &Value, spec_url: Option<&str>) -> Option<String> {
    let server = spec.get("servers")?.as_array()?.first()?;
    let mut url = server.get("url")?.as_str()?.to_string();
    if let Some(vars) = server.get("variables").and_then(Value::as_object) {
        for (name, var) in vars {
            if let Some(default) = var.get("default").and_then(Value::as_str) {
                url = url.replace(&format!("{{{name}}}"), default);
            }
        }
    }
    if url.starts_with("http://") || url.starts_with("https://") {
        return Some(url);
    }
    reqwest::Url::parse(spec_url?).ok()?.join(&url).ok().map(|u| u.to_string())
}

fn deref<'a>(root: &'a Value, mut v: &'a Value) -> &'a Value {
    for _ in 0..REF_DEPTH {
        match v.get("$ref").and_then(Value::as_str).and_then(|r| r.strip_prefix('#')) {
            Some(pointer) => match root.pointer(pointer) {
                Some(target) => v = target,
                None => return v,
            },
            None => return v,
        }
    }
    v
}

fn inline(root: &Value, v: &Value, refs_left: usize) -> Value {
    if let Some(pointer) = v.get("$ref").and_then(Value::as_str).and_then(|r| r.strip_prefix('#')) {
        let Some(target) = root.pointer(pointer).filter(|_| refs_left > 0) else {
            return json!({ "description": format!("see {pointer} in the spec") });
        };
        return inline(root, target, refs_left - 1);
    }
    match v {
        Value::Object(obj) => Value::Object(
            obj.iter()
                .filter(|(k, _)| !matches!(k.as_str(), "example" | "examples" | "xml" | "externalDocs"))
                .map(|(k, val)| (k.clone(), inline(root, val, refs_left)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(|i| inline(root, i, refs_left)).collect()),
        other => other.clone(),
    }
}

const METHODS: [&str; 8] = ["get", "put", "post", "delete", "options", "head", "patch", "trace"];

fn commands(spec: &Value) -> Result<Vec<Command>> {
    let paths = spec.get("paths").and_then(Value::as_object).ok_or_else(|| anyhow!("the spec has no paths"))?;
    let mut out = Vec::new();
    for (path, item) in paths {
        let item = deref(spec, item);
        let shared: Vec<Value> = item.get("parameters").and_then(Value::as_array).cloned().unwrap_or_default();
        for method in METHODS {
            let Some(op) = item.get(method) else { continue };
            out.push(command(spec, path, method, op, &shared));
        }
    }
    Ok(out)
}

fn command(spec: &Value, path: &str, method: &str, op: &Value, shared: &[Value]) -> Command {
    let original = op
        .get("operationId")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| {
            let slug: Vec<&str> = path
                .split('/')
                .filter(|s| !s.is_empty())
                .map(|s| s.trim_start_matches('{').trim_end_matches('}'))
                .collect();
            format!("{method}-{}", slug.join("-"))
        });
    let mut properties = Map::new();
    let mut required = Vec::new();
    let mut params = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let own: Vec<Value> = op.get("parameters").and_then(Value::as_array).cloned().unwrap_or_default();
    for raw in own.iter().chain(shared.iter()) {
        let p = deref(spec, raw);
        let (Some(name), Some(at)) = (p.get("name").and_then(Value::as_str), p.get("in").and_then(Value::as_str)) else {
            continue;
        };
        let at = match at {
            "path" => ParamAt::Path,
            "query" => ParamAt::Query,
            "header" => ParamAt::Header,
            _ => continue,
        };
        if !seen.insert((name.to_string(), at)) {
            continue;
        }
        let mut schema = p.get("schema").map(|s| inline(spec, s, REF_DEPTH)).unwrap_or_else(|| json!({ "type": "string" }));
        if let (Some(d), Value::Object(obj)) = (p.get("description").and_then(Value::as_str), &mut schema) {
            obj.entry("description").or_insert_with(|| json!(d));
        }
        let flag = if properties.contains_key(name) { format!("{}-{name}", at_word(at)) } else { name.to_string() };
        if at == ParamAt::Path || p.get("required").and_then(Value::as_bool) == Some(true) {
            required.push(flag.clone());
        }
        properties.insert(flag.clone(), schema);
        params.push(HttpParam { name: name.to_string(), flag, at });
    }
    let body = match op.get("requestBody").map(|b| deref(spec, b)) {
        None => Body::None,
        Some(rb) => {
            let content = rb.get("content").and_then(Value::as_object);
            let (content_type, media) = content
                .and_then(|c| c.iter().find(|(t, _)| t.contains("json")).or_else(|| c.iter().next()))
                .map(|(t, m)| (t.clone(), m.clone()))
                .unwrap_or_else(|| ("application/json".into(), json!({})));
            let schema = media.get("schema").map(|s| inline(spec, s, REF_DEPTH)).unwrap_or_else(|| json!({}));
            let body_required = rb.get("required").and_then(Value::as_bool) == Some(true);
            let fields = schema.get("properties").and_then(Value::as_object).filter(|_| content_type.contains("json"));
            match fields {
                Some(fields) if !fields.is_empty() => {
                    let inner_required: Vec<String> = schema
                        .get("required")
                        .and_then(Value::as_array)
                        .map(|r| r.iter().filter_map(Value::as_str).map(str::to_string).collect())
                        .unwrap_or_default();
                    let mut pairs = Vec::new();
                    for (prop, s) in fields {
                        let flag = if properties.contains_key(prop) { format!("body-{prop}") } else { prop.clone() };
                        if body_required && inner_required.contains(prop) {
                            required.push(flag.clone());
                        }
                        properties.insert(flag.clone(), s.clone());
                        pairs.push((flag, prop.clone()));
                    }
                    Body::Fields { content_type, fields: pairs }
                }
                _ => {
                    let mut s = if content_type.contains("json") { schema } else { json!({ "type": "string" }) };
                    if let Value::Object(obj) = &mut s {
                        obj.entry("description").or_insert_with(|| json!(format!("the request body, {content_type}")));
                    }
                    if body_required {
                        required.push("body".into());
                    }
                    properties.insert("body".into(), s);
                    Body::Whole { content_type, flag: "body".into() }
                }
            }
        }
    };
    let summary = op.get("summary").and_then(Value::as_str).map(first_line).unwrap_or_default();
    let description = op.get("description").and_then(Value::as_str).map(|d| clip(d.trim(), 2000)).unwrap_or_default();
    Command {
        name: kebab(&original),
        original,
        summary: if summary.is_empty() { first_line(&description) } else { summary },
        description,
        group: op.get("tags").and_then(Value::as_array).and_then(|t| t.first()).and_then(Value::as_str).map(str::to_string),
        write: !matches!(method, "get" | "head" | "options"),
        input: json!({ "type": "object", "properties": properties, "required": required }),
        target: Target::Http(HttpOp { method: method.to_string(), path: path.to_string(), params, body }),
    }
}

fn at_word(at: ParamAt) -> &'static str {
    match at {
        ParamAt::Path => "path",
        ParamAt::Query => "query",
        ParamAt::Header => "header",
    }
}

fn auth_hint(spec: &Value) -> Option<AuthHint> {
    let schemes = spec.pointer("/components/securitySchemes").and_then(Value::as_object)?;
    let named: Option<String> = spec
        .get("security")
        .and_then(Value::as_array)
        .and_then(|reqs| reqs.iter().filter_map(Value::as_object).flat_map(|r| r.keys()).next())
        .cloned();
    let scheme = match named.as_deref().and_then(|n| schemes.get(n)) {
        Some(s) => deref(spec, s),
        None => deref(spec, schemes.values().next()?),
    };
    match scheme.get("type").and_then(Value::as_str)? {
        "http" => match scheme.get("scheme").and_then(Value::as_str).map(str::to_ascii_lowercase).as_deref() {
            Some("basic") => Some(AuthHint::Basic),
            _ => Some(AuthHint::Bearer),
        },
        "apiKey" => {
            let name = scheme.get("name").and_then(Value::as_str)?.to_string();
            match scheme.get("in").and_then(Value::as_str)? {
                "header" if name.eq_ignore_ascii_case("authorization") => Some(AuthHint::Bearer),
                "header" => Some(AuthHint::Header(name)),
                "query" => Some(AuthHint::Query(name)),
                _ => None,
            }
        }
        "oauth2" | "openIdConnect" => {
            let flow = scheme.pointer("/flows/authorizationCode");
            let text = |k: &str| flow.and_then(|f| f.get(k)).and_then(Value::as_str).map(str::to_string);
            let scopes = flow
                .and_then(|f| f.get("scopes"))
                .and_then(Value::as_object)
                .map(|s| s.keys().cloned().collect())
                .unwrap_or_default();
            Some(AuthHint::OAuth { authorize_url: text("authorizationUrl"), token_url: text("tokenUrl"), scopes })
        }
        _ => None,
    }
}

pub fn fill_oauth(app: &OAuthApp, hint: Option<&AuthHint>) -> OAuthApp {
    let mut app = app.clone();
    if let Some(AuthHint::OAuth { authorize_url, token_url, scopes }) = hint {
        if app.authorize_url.is_none() {
            app.authorize_url = authorize_url.clone();
        }
        if app.token_url.is_none() {
            app.token_url = token_url.clone();
        }
        if app.scopes.is_empty() {
            app.scopes = scopes.clone();
        }
    }
    app
}

#[derive(Debug)]
pub struct Reply {
    pub status: u16,
    pub body: Value,
    pub text: Option<String>,
}

fn encode_segment(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn as_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

pub async fn call(
    http: &reqwest::Client,
    base: &str,
    op: &HttpOp,
    args: &Map<String, Value>,
    applied: &[Applied],
    timeout: Duration,
) -> Result<Reply> {
    let mut path = op.path.clone();
    let mut query: Vec<(String, String)> = Vec::new();
    let mut headers: Vec<(String, String)> = Vec::new();
    for p in &op.params {
        let Some(v) = args.get(&p.flag) else { continue };
        match p.at {
            ParamAt::Path => path = path.replace(&format!("{{{}}}", p.name), &encode_segment(&as_text(v))),
            ParamAt::Query => match v {
                Value::Array(items) => query.extend(items.iter().map(|i| (p.name.clone(), as_text(i)))),
                other => query.push((p.name.clone(), as_text(other))),
            },
            ParamAt::Header => headers.push((p.name.clone(), as_text(v))),
        }
    }
    for a in applied {
        match a {
            Applied::Header(k, v) => headers.push((k.clone(), v.clone())),
            Applied::Query(k, v) => query.push((k.clone(), v.clone())),
        }
    }
    let url = format!("{base}{path}");
    let method = reqwest::Method::from_bytes(op.method.to_uppercase().as_bytes()).context("method")?;
    let mut req = http.request(method, &url).timeout(timeout).header("Accept", "application/json, */*;q=0.5");
    if !query.is_empty() {
        req = req.query(&query);
    }
    for (k, v) in &headers {
        req = req.header(k.as_str(), v.as_str());
    }
    match &op.body {
        Body::None => {}
        Body::Fields { content_type, fields } => {
            let obj: Map<String, Value> = fields
                .iter()
                .filter_map(|(flag, prop)| args.get(flag).map(|v| (prop.clone(), v.clone())))
                .collect();
            if !obj.is_empty() {
                req = req.header("Content-Type", content_type.as_str()).body(Value::Object(obj).to_string());
            }
        }
        Body::Whole { content_type, flag } => {
            if let Some(v) = args.get(flag) {
                let body = if content_type.contains("json") { v.to_string() } else { as_text(v) };
                req = req.header("Content-Type", content_type.as_str()).body(body);
            }
        }
    }
    let resp = req.send().await.map_err(|e| anyhow!("{url}: {}", without_url(&e)))?;
    let status = resp.status().as_u16();
    let content_type = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let bytes = resp.bytes().await.map_err(|e| anyhow!("{url}: reading the answer: {}", without_url(&e)))?;
    if bytes.is_empty() {
        return Ok(Reply { status, body: Value::Null, text: Some(String::new()) });
    }
    if let Ok(v) = serde_json::from_slice::<Value>(&bytes) {
        return Ok(Reply { status, body: v, text: None });
    }
    match std::str::from_utf8(&bytes) {
        Ok(t) => Ok(Reply { status, body: Value::Null, text: Some(t.to_string()) }),
        Err(_) => Ok(Reply {
            status,
            body: Value::Null,
            text: Some(format!("[{} bytes of {}]", bytes.len(), if content_type.is_empty() { "binary" } else { &content_type })),
        }),
    }
}

fn without_url(e: &reqwest::Error) -> String {
    let text = e.to_string();
    match text.split_once(" for url (") {
        Some((head, _)) => head.to_string(),
        None => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn petstore() -> Value {
        json!({
            "openapi": "3.0.3",
            "info": { "title": "Petstore" },
            "servers": [{ "url": "https://{region}.pets.example/v1", "variables": { "region": { "default": "eu" } } }],
            "security": [{ "key": [] }],
            "components": {
                "securitySchemes": { "key": { "type": "apiKey", "in": "header", "name": "X-Api-Key" } },
                "parameters": { "PetId": { "name": "petId", "in": "path", "required": true, "schema": { "type": "integer" }, "description": "which pet" } },
                "schemas": {
                    "NewPet": { "type": "object", "required": ["name"], "properties": {
                        "name": { "type": "string" }, "tag": { "type": "string" }, "owner": { "$ref": "#/components/schemas/Owner" } } },
                    "Owner": { "type": "object", "properties": { "name": { "type": "string" }, "pets": { "type": "array", "items": { "$ref": "#/components/schemas/NewPet" } } } }
                }
            },
            "paths": {
                "/pets": {
                    "get": { "operationId": "listPets", "tags": ["pets"], "summary": "List all pets",
                        "parameters": [{ "name": "limit", "in": "query", "schema": { "type": "integer" } },
                                       { "name": "tag", "in": "query", "schema": { "type": "array", "items": { "type": "string" } } }] },
                    "post": { "operationId": "createPet", "tags": ["pets"], "summary": "Create a pet",
                        "requestBody": { "required": true, "content": { "application/json": { "schema": { "$ref": "#/components/schemas/NewPet" } } } } }
                },
                "/pets/{petId}": {
                    "parameters": [{ "$ref": "#/components/parameters/PetId" }],
                    "get": { "operationId": "showPetById", "tags": ["pets"], "summary": "Info for a pet" },
                    "delete": { "tags": ["pets"], "description": "Remove a pet.\nIt is gone." }
                },
                "/notes": {
                    "put": { "operationId": "putNotes", "requestBody": { "content": { "text/plain": { "schema": { "type": "string" } } } } }
                }
            }
        })
    }

    fn loaded() -> Loaded {
        read(&petstore(), "petstore.json", None, None).unwrap()
    }

    #[test]
    fn every_operation_is_a_command_named_and_grouped() {
        let l = loaded();
        assert_eq!(l.base, "https://eu.pets.example/v1");
        assert_eq!(l.title.as_deref(), Some("Petstore"));
        let names: Vec<&str> = l.catalogue.commands.iter().map(|c| c.name.as_str()).collect();
        for n in ["list-pets", "create-pet", "show-pet-by-id", "delete-pets-pet-id", "put-notes"] {
            assert!(names.contains(&n), "{n} in {names:?}");
        }
        let del = l.catalogue.find("delete-pets-petid").unwrap();
        assert!(del.write);
        assert_eq!(del.summary, "Remove a pet.");
        assert!(!l.catalogue.find("list-pets").unwrap().write);
        assert!(l.catalogue.groups().contains(&("pets".to_string(), 4)), "{:?}", l.catalogue.groups());
    }

    #[test]
    fn parameters_and_body_fields_are_flags_with_refs_followed() {
        let l = loaded();
        let show = l.catalogue.find("show-pet-by-id").unwrap();
        assert_eq!(show.required(), ["petId"]);
        assert_eq!(show.input["properties"]["petId"]["type"], "integer");
        assert_eq!(show.input["properties"]["petId"]["description"], "which pet");
        let create = l.catalogue.find("create-pet").unwrap();
        assert_eq!(create.required(), ["name"]);
        assert_eq!(create.input["properties"]["owner"]["properties"]["pets"]["items"]["type"], "object");
        let notes = l.catalogue.find("put-notes").unwrap();
        assert_eq!(notes.input["properties"]["body"]["type"], "string");
    }

    #[test]
    fn the_spec_says_how_it_wants_to_be_signed_in() {
        assert_eq!(loaded().auth_hint, Some(AuthHint::Header("X-Api-Key".into())));
        let mut spec = petstore();
        spec["components"]["securitySchemes"] = json!({ "gh": { "type": "oauth2", "flows": { "authorizationCode": {
            "authorizationUrl": "https://example.com/auth", "tokenUrl": "https://example.com/token", "scopes": { "repo": "" } } } } });
        spec["security"] = json!([{ "gh": ["repo"] }]);
        let hint = read(&spec, "x", None, None).unwrap().auth_hint;
        let app = fill_oauth(
            &OAuthApp { client_id: "c".into(), client_secret: None, authorize_url: None, token_url: None, scopes: vec![], redirect_port: 1 },
            hint.as_ref(),
        );
        assert_eq!(app.token_url.as_deref(), Some("https://example.com/token"));
        assert_eq!(app.scopes, ["repo"]);
    }

    #[test]
    fn swagger_two_and_serverless_specs_say_what_to_do() {
        assert!(read(&json!({ "swagger": "2.0" }), "old", None, None).unwrap_err().to_string().contains("convert"));
        let mut spec = petstore();
        spec.as_object_mut().unwrap().remove("servers");
        assert!(read(&spec, "s", None, None).unwrap_err().to_string().contains("set `base`"));
        assert_eq!(read(&spec, "s", Some("http://localhost:1/"), None).unwrap().base, "http://localhost:1");
        let rel = read(&petstore(), "s", None, Some("https://api.example.com/spec/openapi.json")).unwrap();
        assert_eq!(rel.base, "https://eu.pets.example/v1");
    }

    #[test]
    fn a_path_segment_is_percent_encoded() {
        assert_eq!(encode_segment("a b/c"), "a%20b%2Fc");
        assert_eq!(encode_segment("plain-1.2_~"), "plain-1.2_~");
    }
}
