//! wrepl end to end: the `web2` binary, in a home of its own, against an
//! OpenAPI service served here and an MCP server over stdio and HTTP.

use std::path::PathBuf;
use std::process::Stdio;

use axum::Json;
use axum::extract::{Path, RawQuery};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

struct Home {
    dir: tempfile::TempDir,
    env: Vec<(String, String)>,
}

impl Home {
    fn new(config: Value) -> Home {
        let home = Home { dir: tempfile::tempdir().unwrap(), env: Vec::new() };
        home.configure(config);
        home
    }

    fn configure(&self, config: Value) {
        std::fs::write(self.dir.path().join("web2.json"), serde_json::to_string_pretty(&config).unwrap()).unwrap();
    }

    fn with_env(mut self, key: &str, value: &str) -> Home {
        self.env.push((key.into(), value.into()));
        self
    }

    fn command(&self, args: &[&str]) -> tokio::process::Command {
        let d = self.dir.path();
        let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_web2"));
        cmd.args(args)
            .env("WEB2_CONFIG", d.join("web2.json"))
            .env("WEB2_STATE", d.join("state"))
            .env("HOME", d)
            .env("XDG_STATE_HOME", d.join("xdg-state"))
            .env("XDG_CONFIG_HOME", d.join("xdg-config"))
            .env_remove("RUST_LOG")
            .envs(self.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd
    }

    async fn run(&self, line: &str) -> (bool, String) {
        let out = self.command(&["run", line]).output().await.unwrap();
        let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        (out.status.success(), text.trim_end().to_string())
    }

    async fn ok(&self, line: &str) -> String {
        let (ok, text) = self.run(line).await;
        assert!(ok, "`{line}` failed:\n{text}");
        text
    }

    async fn fails(&self, line: &str) -> String {
        let (ok, text) = self.run(line).await;
        assert!(!ok, "`{line}` should have failed:\n{text}");
        text
    }

    async fn login(&self, service: &str, token: &str) -> String {
        let mut child = self.command(&["login", service, "--token-stdin"]).spawn().unwrap();
        child.stdin.take().unwrap().write_all(token.as_bytes()).await.unwrap();
        let out = child.wait_with_output().await.unwrap();
        assert!(out.status.success(), "login failed: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).to_string()
    }

    async fn logout(&self, service: &str) {
        let out = self.command(&["logout", service]).output().await.unwrap();
        assert!(out.status.success());
    }
}

fn pets() -> Vec<Value> {
    vec![json!({ "id": 1, "name": "rex", "tag": "dog" }), json!({ "id": 2, "name": "tom", "tag": "cat" })]
}

fn unkeyed(headers: &HeaderMap) -> Option<Response> {
    match headers.get("x-api-key").and_then(|v| v.to_str().ok()) {
        Some("k3y") => None,
        _ => Some((StatusCode::UNAUTHORIZED, Json(json!({ "message": "bad key" }))).into_response()),
    }
}

async fn pet_shop() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let spec = json!({
        "openapi": "3.0.3",
        "info": { "title": "Pet Shop" },
        "servers": [{ "url": format!("http://127.0.0.1:{port}/v1") }],
        "security": [{ "key": [] }],
        "components": {
            "securitySchemes": { "key": { "type": "apiKey", "in": "header", "name": "X-Api-Key" } },
            "schemas": { "NewPet": { "type": "object", "required": ["name"], "properties": { "name": { "type": "string" }, "tag": { "type": "string" } } } }
        },
        "paths": {
            "/pets": {
                "get": { "operationId": "listPets", "tags": ["pets"], "summary": "List the pets",
                    "parameters": [{ "name": "limit", "in": "query", "schema": { "type": "integer" } },
                                   { "name": "tag", "in": "query", "schema": { "type": "array", "items": { "type": "string" } } }] },
                "post": { "operationId": "createPet", "tags": ["pets"], "summary": "Add a pet",
                    "requestBody": { "required": true, "content": { "application/json": { "schema": { "$ref": "#/components/schemas/NewPet" } } } } }
            },
            "/pets/{petId}": {
                "parameters": [{ "name": "petId", "in": "path", "required": true, "schema": { "type": "integer" } }],
                "get": { "operationId": "showPetById", "tags": ["pets"], "summary": "One pet" },
                "delete": { "operationId": "deletePet", "tags": ["pets"], "summary": "Let a pet go" }
            }
        }
    });
    let app = axum::Router::new()
        .route("/openapi.json", get(move || async move { Json(spec) }))
        .route(
            "/v1/pets",
            get(|headers: HeaderMap, RawQuery(q): RawQuery| async move {
                if let Some(r) = unkeyed(&headers) {
                    return r;
                }
                let pairs: Vec<(String, String)> = q
                    .map(|q| url_pairs(&q))
                    .unwrap_or_default();
                let tags: Vec<&str> = pairs.iter().filter(|(k, _)| k == "tag").map(|(_, v)| v.as_str()).collect();
                let limit = pairs.iter().find(|(k, _)| k == "limit").and_then(|(_, v)| v.parse::<usize>().ok()).unwrap_or(usize::MAX);
                let list: Vec<Value> = pets()
                    .into_iter()
                    .filter(|p| tags.is_empty() || tags.contains(&p["tag"].as_str().unwrap()))
                    .take(limit)
                    .collect();
                Json(Value::Array(list)).into_response()
            })
            .post(|headers: HeaderMap, Json(body): Json<Value>| async move {
                if let Some(r) = unkeyed(&headers) {
                    return r;
                }
                let mut pet = body;
                pet["id"] = json!(3);
                (StatusCode::CREATED, Json(pet)).into_response()
            }),
        )
        .route(
            "/v1/pets/{pet_id}",
            get(|headers: HeaderMap, Path(id): Path<u64>| async move {
                if let Some(r) = unkeyed(&headers) {
                    return r;
                }
                match pets().into_iter().find(|p| p["id"] == id) {
                    Some(p) => Json(p).into_response(),
                    None => (StatusCode::NOT_FOUND, Json(json!({ "message": "no such pet" }))).into_response(),
                }
            })
            .delete(|| async { StatusCode::NO_CONTENT }),
        );
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    port
}

fn url_pairs(q: &str) -> Vec<(String, String)> {
    q.split('&')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.to_string(), v.replace("%20", " ")))
        .collect()
}

fn mock_mcp() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let path = exe.parent().unwrap().parent().unwrap().join("examples").join("mock_mcp");
    assert!(path.exists(), "build the mock first: cargo build --example mock_mcp ({})", path.display());
    path
}

#[tokio::test(flavor = "multi_thread")]
async fn an_openapi_service_is_a_cli_that_only_reads_until_told() {
    let port = pet_shop().await;
    let spec = format!("http://127.0.0.1:{port}/openapi.json");
    let home = Home::new(json!({ "services": { "pets": { "openapi": spec, "description": "the pet shop" } } }));

    let help = home.ok("help").await;
    assert!(help.contains("pets") && help.contains("the pet shop") && help.contains("reads only"), "{help}");

    let unsigned = home.fails("pets list-pets").await;
    assert!(unsigned.contains("web2 login pets"), "{unsigned}");
    assert!(home.login("pets", "k3y\n").await.contains("stored"));

    let listing = home.ok("pets help").await;
    assert!(listing.contains("list-pets") && listing.contains("create-pet"), "{listing}");
    assert!(listing.contains("Add a pet (write, off)"), "{listing}");

    assert_eq!(home.ok("pets list-pets --limit 1 --jq '.[].name'").await, "rex");
    assert_eq!(home.ok("pets list-pets --tag cat --tag dog --jq 'map(.name) | join(\",\")'").await, "rex,tom");
    assert_eq!(home.ok("pets show-pet-by-id 2 --jq .name").await, "tom");
    let usage = home.ok("pets show-pet-by-id --help").await;
    assert!(usage.contains("GET /pets/{petId}") && usage.contains("--petId <integer>") && usage.contains("required"), "{usage}");

    let missing = home.fails("pets show-pet-by-id 99").await;
    assert!(missing.contains("HTTP 404") && missing.contains("no such pet"), "{missing}");
    let mistyped = home.fails("pets list-petz").await;
    assert!(mistyped.contains("did you mean list-pets"), "{mistyped}");

    let refused = home.fails("pets create-pet --name kit").await;
    assert!(refused.contains("only reads here") && refused.contains("services.pets.allow"), "{refused}");

    home.configure(json!({ "services": { "pets": { "openapi": spec, "allow": ["create-pet"] } } }));
    assert_eq!(home.ok("pets create-pet --name kit --tag fox --jq '.id, .name'").await, "3\nkit");
    assert!(home.fails("pets delete-pet 1").await.contains("only reads here"));

    home.logout("pets").await;
    assert!(home.fails("pets list-pets").await.contains("web2 login pets"));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_mcp_server_over_stdio_is_a_cli_too() {
    let server = mock_mcp();
    let home = Home::new(json!({ "mcpServers": { "mock": { "command": server } } }));

    let listing = home.ok("mock help").await;
    for name in ["echo", "add", "explode", "create-note"] {
        assert!(listing.contains(name), "{name} in {listing}");
    }
    assert!(listing.contains("Create a note (write, off)"), "{listing}");

    assert_eq!(home.ok("mock echo 'hello there'").await, "hello there");
    assert_eq!(home.ok("mock add --a 2 --b 3 --jq .sum").await, "5");
    assert!(home.fails("mock create-note --title x").await.contains("only reads here"));
    assert!(home.fails("mock explode").await.contains("it broke"));
    assert!(home.fails("mock nope").await.contains("has no command nope"));
    assert!(home.fails("mock add --a two --b 3").await.contains("whole number"));

    home.configure(json!({ "mcpServers": { "mock": { "command": server } }, "services": { "mock": { "writes": true } } }));
    assert_eq!(home.ok("mock create_note --title x").await, "created x");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_mcp_server_over_http_gets_its_bearer_and_nothing_leaks_without_it() {
    let mut child = tokio::process::Command::new(mock_mcp())
        .args(["--http", "--token", "s3cret"])
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let port = lines.next_line().await.unwrap().unwrap().trim_start_matches("listening ").to_string();
    let config = json!({ "services": { "remote": {
        "mcp": { "url": format!("http://127.0.0.1:{port}/mcp") },
        "auth": { "bearer": { "env": "MOCK_TOKEN" } } } } });

    let unset = Home::new(config.clone()).fails("remote echo hi").await;
    assert!(unset.contains("$MOCK_TOKEN"), "{unset}");
    let wrong = Home::new(config.clone()).with_env("MOCK_TOKEN", "nope").fails("remote echo hi").await;
    assert!(wrong.contains("remote"), "{wrong}");
    let right = Home::new(config).with_env("MOCK_TOKEN", "s3cret");
    assert_eq!(right.ok("remote echo hi").await, "hi");
    assert_eq!(right.ok("remote add 40 2 --jq .sum").await, "42");
    drop(child);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_broken_or_empty_config_says_what_to_do() {
    let home = Home::new(json!({}));
    assert!(home.ok("help").await.contains("reaches no services yet"));
    assert!(home.fails("github repos").await.contains("no service is called github"));
    home.configure(json!({ "services": { "x": { "openapi": "a.json", "colour": "red" } } }));
    let (ok, text) = home.run("help").await;
    assert!(!ok && text.contains("services.x"), "{text}");
}
