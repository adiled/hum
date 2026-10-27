//! An MCP server for web2's tests: four tools, over stdio, or over
//! streamable HTTP with `--http`, printing the port it listens on. With
//! `--token <t>` it refuses a request whose bearer is not `t`, and over
//! stdio it does not start unless `$MOCK_TOKEN` is `t`.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ServiceExt, schemars, tool, tool_router};

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct Echo {
    text: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct Add {
    a: i64,
    b: i64,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct Note {
    title: String,
}

#[derive(Debug, Clone)]
struct Mock;

#[tool_router(server_handler)]
impl Mock {
    #[tool(description = "Say the text back", annotations(read_only_hint = true))]
    fn echo(&self, Parameters(Echo { text }): Parameters<Echo>) -> String {
        text
    }

    #[tool(description = "Add two numbers, answered as JSON", annotations(read_only_hint = true))]
    fn add(&self, Parameters(Add { a, b }): Parameters<Add>) -> String {
        format!("{{\"sum\": {}}}", a + b)
    }

    #[tool(description = "Create a note")]
    fn create_note(&self, Parameters(Note { title }): Parameters<Note>) -> String {
        format!("created {title}")
    }

    #[tool(description = "Always fails", annotations(read_only_hint = true))]
    fn explode(&self) -> CallToolResult {
        CallToolResult::error(vec![ContentBlock::text("it broke")])
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let token = args.iter().position(|a| a == "--token").and_then(|i| args.get(i + 1)).cloned();
    if !args.iter().any(|a| a == "--http") {
        if let Some(t) = &token
            && std::env::var("MOCK_TOKEN").ok().as_ref() != Some(t)
        {
            anyhow::bail!("$MOCK_TOKEN is not the token");
        }
        let running = Mock.serve(rmcp::transport::stdio()).await?;
        running.waiting().await?;
        return Ok(());
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    let service = StreamableHttpService::new(|| Ok(Mock), LocalSessionManager::default().into(), StreamableHttpServerConfig::default());
    let router = if args.iter().any(|a| a == "--oauth") {
        oauth::router(&base, service)
    } else {
        let guard = axum::middleware::from_fn(move |req: axum::extract::Request, next: axum::middleware::Next| {
            let token = token.clone();
            async move {
                let sent = req.headers().get("authorization").and_then(|v| v.to_str().ok()).map(str::to_string);
                match token {
                    Some(t) if sent.as_deref() != Some(format!("Bearer {t}").as_str()) => {
                        axum::response::IntoResponse::into_response(axum::http::StatusCode::UNAUTHORIZED)
                    }
                    _ => next.run(req).await,
                }
            }
        });
        axum::Router::new().nest_service("/mcp", service).layer(guard)
    };
    println!("listening {}", listener.local_addr()?.port());
    axum::serve(listener, router).await?;
    Ok(())
}

mod oauth {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use axum::extract::{Form, Query, Request, State};
    use axum::http::{HeaderValue, StatusCode, header};
    use axum::middleware::Next;
    use axum::response::{IntoResponse, Redirect, Response};
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use base64::Engine as _;
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};

    #[derive(Clone)]
    struct Shared {
        base: String,
        challenge: Arc<Mutex<Option<String>>>,
    }

    fn resource(base: &str) -> Value {
        json!({ "resource": format!("{base}/mcp"), "authorization_servers": [base] })
    }

    fn server(base: &str) -> Value {
        json!({
            "issuer": base,
            "authorization_endpoint": format!("{base}/authorize"),
            "token_endpoint": format!("{base}/token"),
            "registration_endpoint": format!("{base}/register"),
            "response_types_supported": ["code"],
            "grant_types_supported": ["authorization_code", "refresh_token"],
            "code_challenge_methods_supported": ["S256"],
            "token_endpoint_auth_methods_supported": ["none"],
            "authorization_response_iss_parameter_supported": true
        })
    }

    async fn register(Json(req): Json<Value>) -> Response {
        let mut client = json!({
            "client_id": "web2-test-client",
            "token_endpoint_auth_method": "none",
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"]
        });
        client["redirect_uris"] = req.get("redirect_uris").cloned().unwrap_or(json!([]));
        (StatusCode::CREATED, Json(client)).into_response()
    }

    async fn authorize(State(s): State<Shared>, Query(q): Query<HashMap<String, String>>) -> Response {
        *s.challenge.lock().unwrap() = q.get("code_challenge").cloned();
        let (Some(redirect), Some(state)) = (q.get("redirect_uri"), q.get("state")) else {
            return StatusCode::BAD_REQUEST.into_response();
        };
        let mut to = reqwest::Url::parse(redirect).unwrap();
        to.query_pairs_mut().append_pair("code", "c0de").append_pair("state", state).append_pair("iss", &s.base);
        Redirect::to(to.as_str()).into_response()
    }

    async fn token(State(s): State<Shared>, Form(form): Form<HashMap<String, String>>) -> Response {
        let fresh = || json!({ "access_token": "tok-1", "token_type": "Bearer", "expires_in": 3600, "refresh_token": "r-1" });
        match form.get("grant_type").map(String::as_str) {
            Some("authorization_code") => {
                let verifier = form.get("code_verifier").cloned().unwrap_or_default();
                let made = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
                if form.get("code").map(String::as_str) != Some("c0de") || s.challenge.lock().unwrap().as_deref() != Some(made.as_str()) {
                    return (StatusCode::BAD_REQUEST, Json(json!({ "error": "invalid_grant" }))).into_response();
                }
                Json(fresh()).into_response()
            }
            Some("refresh_token") => Json(fresh()).into_response(),
            _ => (StatusCode::BAD_REQUEST, Json(json!({ "error": "unsupported_grant_type" }))).into_response(),
        }
    }

    async fn bearer(State(s): State<Shared>, req: Request, next: Next) -> Response {
        let sent = req.headers().get(header::AUTHORIZATION).and_then(|v| v.to_str().ok());
        if sent == Some("Bearer tok-1") {
            return next.run(req).await;
        }
        let mut refused = StatusCode::UNAUTHORIZED.into_response();
        let challenge = format!("Bearer resource_metadata=\"{}/.well-known/oauth-protected-resource/mcp\"", s.base);
        refused.headers_mut().insert(header::WWW_AUTHENTICATE, HeaderValue::from_str(&challenge).unwrap());
        refused
    }

    pub fn router<S>(base: &str, mcp: S) -> Router
    where
        S: tower_service::Service<Request, Error = std::convert::Infallible> + Clone + Send + Sync + 'static,
        S::Response: IntoResponse,
        S::Future: Send + 'static,
    {
        let shared = Shared { base: base.to_string(), challenge: Arc::new(Mutex::new(None)) };
        let (r, a) = (resource(base), server(base));
        let guarded = Router::new()
            .nest_service("/mcp", mcp)
            .layer(axum::middleware::from_fn_with_state(shared.clone(), bearer));
        Router::new()
            .route("/.well-known/oauth-protected-resource", get({ let r = r.clone(); move || async move { Json(r) } }))
            .route("/.well-known/oauth-protected-resource/mcp", get(move || async move { Json(r) }))
            .route("/.well-known/oauth-authorization-server", get({ let a = a.clone(); move || async move { Json(a) } }))
            .route("/.well-known/oauth-authorization-server/mcp", get(move || async move { Json(a) }))
            .route("/register", post(register))
            .route("/authorize", get(authorize))
            .route("/token", post(token))
            .with_state(shared)
            .merge(guarded)
    }
}
