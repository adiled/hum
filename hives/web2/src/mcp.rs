//! MCP servers, reached as commands.
//!
//! Each server is one connection, opened at its first use and kept while it
//! is used: a child process for a stdio server, a session for one over
//! streamable HTTP. A connection left idle past `idleMs` is closed, and one
//! that drops is opened again on the next call. A server's tools are listed
//! when it connects, and again when it says its list changed. Every
//! connection is closed before web2 exits, so none is cut off mid-close.
//!
//! web2 offers a server no capabilities of its own: no sampling, no
//! elicitation, no roots. A tool that asks for input mid-call, or that
//! turns into a long task, answers with an error saying so. A tool whose
//! annotations call it read-only is a read; every other tool is a write.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use parking_lot::Mutex;
use rmcp::model::{CallToolRequestParams, CallToolResponse, ClientCapabilities, ClientConfig, Implementation};
use rmcp::service::{NotificationContext, Peer, RunningService};
use rmcp::transport::auth::{AuthClient, AuthorizationManager, AuthorizationRequest, OAuthState, StoredCredentials};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::{StreamableHttpClientTransport, TokioChildProcess};
use rmcp::{ClientHandler, RoleClient, ServiceError, ServiceExt};
use serde_json::{Map, Value, json};

use crate::auth::{Applied, Creds, Denied, McpStore, await_callback, needs_login, redirect_uri, show_sign_in};
use crate::catalogue::{Catalogue, Command, Target, first_line, kebab};
use crate::config::{McpServer, Service};

const RELIST_AFTER: Duration = Duration::from_secs(300);

#[derive(Clone)]
pub struct Handler {
    stale: Arc<AtomicBool>,
}

impl ClientHandler for Handler {
    fn get_info(&self) -> ClientConfig {
        ClientConfig::new(ClientCapabilities::default(), Implementation::new("web2", env!("CARGO_PKG_VERSION")))
    }

    async fn on_tool_list_changed(&self, _context: NotificationContext<RoleClient>) {
        self.stale.store(true, Ordering::Relaxed);
    }
}

struct Conn {
    client: RunningService<RoleClient, Handler>,
    catalogue: Catalogue,
    stale: Arc<AtomicBool>,
    listed: Instant,
    used: Instant,
}

type Slot = Arc<tokio::sync::Mutex<Option<Conn>>>;

pub struct Pool {
    slots: Mutex<HashMap<String, Slot>>,
    idle: Duration,
}

pub fn commands(tools: &[Value]) -> Catalogue {
    Catalogue::new(
        tools
            .iter()
            .filter_map(|t| {
                let original = t.get("name").and_then(Value::as_str).filter(|n| !n.is_empty())?.to_string();
                let description = t.get("description").and_then(Value::as_str).unwrap_or("").trim().to_string();
                let title = t
                    .get("title")
                    .or_else(|| t.pointer("/annotations/title"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let read_only = t.pointer("/annotations/readOnlyHint").and_then(Value::as_bool) == Some(true);
                let input = match t.get("inputSchema") {
                    Some(s) if s.is_object() => s.clone(),
                    _ => json!({ "type": "object", "properties": {} }),
                };
                Some(Command {
                    name: kebab(&original),
                    summary: title.unwrap_or_else(|| first_line(&description)),
                    description,
                    group: None,
                    write: !read_only,
                    input,
                    target: Target::Mcp { tool: original.clone() },
                    original,
                })
            })
            .collect(),
    )
}

fn handler() -> (Handler, Arc<AtomicBool>) {
    let stale = Arc::new(AtomicBool::new(false));
    (Handler { stale: stale.clone() }, stale)
}

fn http_client(headers: &[(String, String)]) -> Result<reqwest::Client> {
    let mut map = reqwest::header::HeaderMap::new();
    for (k, v) in headers {
        let name = reqwest::header::HeaderName::from_bytes(k.as_bytes()).with_context(|| format!("header name {k:?}"))?;
        let mut value = reqwest::header::HeaderValue::from_str(v).with_context(|| format!("header {k}"))?;
        value.set_sensitive(true);
        map.insert(name, value);
    }
    Ok(reqwest::Client::builder().default_headers(map).build()?)
}

enum Opened {
    Ready(RunningService<RoleClient, Handler>),
    Challenged,
}

async fn open(service: &Service, server: &McpServer, applied: &[Applied], creds: &Arc<Creds>, handler: Handler) -> Result<Opened, Denied> {
    let failed = |e: String| Denied::Failed(format!("{}: {e}", service.name));
    match server {
        McpServer::Stdio { command, args, env, cwd } => {
            let mut cmd = tokio::process::Command::new(command);
            cmd.args(args).envs(env);
            if let Some(dir) = cwd {
                cmd.current_dir(dir);
            }
            let stderr = if std::env::var_os("WEB2_SERVER_STDERR").is_some() { Stdio::inherit() } else { Stdio::null() };
            let (transport, _) = TokioChildProcess::builder(cmd)
                .stderr(stderr)
                .spawn()
                .map_err(|e| failed(format!("could not start `{command}`: {e}")))?;
            handler.serve(transport).await.map(Opened::Ready).map_err(|e| failed(format!("did not start as an MCP server: {e}")))
        }
        McpServer::Http { url, headers } => {
            let mut all: Vec<(String, String)> = headers.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            for a in applied {
                match a {
                    Applied::Header(k, v) => all.push((k.clone(), v.clone())),
                    Applied::Query(..) => return Err(failed("an MCP server takes its key in a header, not the query".into())),
                }
            }
            let client = http_client(&all).map_err(|e| failed(e.to_string()))?;
            let config = StreamableHttpClientTransportConfig::with_uri(url.as_str());
            let stored = creds.get(&service.name).is_some_and(|e| e.get("mcpOauth").is_some());
            let result = if stored && applied.is_empty() {
                let mut manager = AuthorizationManager::new(url.as_str()).await.map_err(|e| failed(format!("sign-in: {e}")))?;
                manager.set_credential_store(McpStore { creds: creds.clone(), service: service.name.clone() });
                if !manager.initialize_from_store().await.map_err(|e| failed(format!("sign-in: {e}")))? {
                    return Err(needs_login(&service.name, "signed in to another authorization server, or the sign-in was dropped"));
                }
                handler.serve(StreamableHttpClientTransport::with_client(AuthClient::new(client, manager), config)).await
            } else {
                handler.serve(StreamableHttpClientTransport::with_client(client, config)).await
            };
            match result {
                Ok(running) => Ok(Opened::Ready(running)),
                Err(e) if e.auth_challenge().is_some() => Ok(Opened::Challenged),
                Err(e) => Err(failed(format!("could not open a session: {e}"))),
            }
        }
    }
}

async fn list(peer: &Peer<RoleClient>, service: &str) -> Result<Catalogue, Denied> {
    let tools = peer
        .list_all_tools()
        .await
        .map_err(|e| Denied::Failed(format!("{service} would not list its tools: {e}")))?;
    let tools: Vec<Value> = tools.iter().filter_map(|t| serde_json::to_value(t).ok()).collect();
    Ok(commands(&tools))
}

impl Pool {
    pub fn new(idle: Duration) -> Pool {
        Pool { slots: Mutex::new(HashMap::new()), idle }
    }

    fn slot(&self, name: &str) -> Slot {
        self.slots.lock().entry(name.to_string()).or_default().clone()
    }

    pub async fn catalogue(&self, service: &Service, server: &McpServer, applied: &[Applied], creds: &Arc<Creds>) -> Result<(Catalogue, Peer<RoleClient>), Denied> {
        let slot = self.slot(&service.name);
        let mut guard = slot.lock().await;
        if guard.as_ref().is_some_and(|c| c.client.is_transport_closed()) {
            *guard = None;
        }
        if guard.is_none() {
            let (handler, stale) = handler();
            let running = match open(service, server, applied, creds, handler).await? {
                Opened::Ready(r) => r,
                Opened::Challenged => return Err(needs_login(&service.name, "asks to be signed in")),
            };
            let catalogue = match list(running.peer(), &service.name).await {
                Ok(c) => c,
                Err(e) => {
                    let _ = running.cancel().await;
                    return Err(e);
                }
            };
            *guard = Some(Conn { client: running, catalogue, stale, listed: Instant::now(), used: Instant::now() });
        }
        let conn = guard.as_mut().ok_or_else(|| Denied::Failed(format!("{} has no connection", service.name)))?;
        if conn.stale.swap(false, Ordering::Relaxed) || conn.listed.elapsed() > RELIST_AFTER {
            if let Ok(catalogue) = list(conn.client.peer(), &service.name).await {
                conn.catalogue = catalogue;
            }
            conn.listed = Instant::now();
        }
        conn.used = Instant::now();
        Ok((conn.catalogue.clone(), conn.client.peer().clone()))
    }

    pub async fn relist(&self, service: &str) {
        let slot = self.slot(service);
        let guard = slot.lock().await;
        if let Some(conn) = guard.as_ref() {
            conn.stale.store(true, Ordering::Relaxed);
        }
    }

    pub async fn drop_conn(&self, service: &str) {
        let slot = self.slot(service);
        let taken = slot.lock().await.take();
        if let Some(conn) = taken {
            let _ = conn.client.cancel().await;
        }
    }

    pub async fn close(&self) {
        let slots: Vec<Slot> = self.slots.lock().drain().map(|(_, slot)| slot).collect();
        for slot in slots {
            let taken = slot.lock().await.take();
            if let Some(conn) = taken {
                let _ = conn.client.cancel().await;
            }
        }
    }

    pub async fn call(
        &self,
        service: &Service,
        server: &McpServer,
        applied: &[Applied],
        creds: &Arc<Creds>,
        tool: &str,
        args: Map<String, Value>,
    ) -> Result<Value, Denied> {
        for attempt in 0..2 {
            let (_, peer) = self.catalogue(service, server, applied, creds).await?;
            let params = CallToolRequestParams::new(tool.to_string()).with_arguments(args.clone());
            match peer.call_tool_once(params).await {
                Ok(CallToolResponse::Complete(result)) => {
                    return serde_json::to_value(&result).map_err(|e| Denied::Failed(e.to_string()));
                }
                Ok(CallToolResponse::InputRequired(_)) => {
                    return Err(Denied::Failed(format!(
                        "{} {tool} asks for more input mid-call, which wrepl does not give; try the call with more of its flags set",
                        service.name
                    )));
                }
                Ok(CallToolResponse::Task(_)) => {
                    return Err(Denied::Failed(format!("{} {tool} runs as a long task, which wrepl does not follow yet", service.name)));
                }
                Ok(_) => return Err(Denied::Failed(format!("{} {tool} answered in a shape wrepl does not read", service.name))),
                Err(ServiceError::TransportClosed | ServiceError::TransportSend(_)) if attempt == 0 => {
                    self.drop_conn(&service.name).await;
                }
                Err(e) => return Err(Denied::Failed(format!("{} {tool}: {e}", service.name))),
            }
        }
        Err(Denied::Failed(format!("{} dropped the connection twice", service.name)))
    }

    pub async fn reap(&self) {
        let slots: Vec<Slot> = self.slots.lock().values().cloned().collect();
        for slot in slots {
            let Ok(mut guard) = slot.try_lock() else { continue };
            if guard.as_ref().is_some_and(|c| c.used.elapsed() > self.idle || c.client.is_transport_closed())
                && let Some(conn) = guard.take()
            {
                drop(guard);
                let _ = conn.client.cancel().await;
            }
        }
    }
}

pub async fn login(service: &Service, server: &McpServer, creds: &Arc<Creds>, port: u16) -> Result<String> {
    let McpServer::Http { url, headers } = server else {
        bail!("{} runs here as a child process; it signs in with whatever its own `env` carries", service.name);
    };
    let plain: Vec<(String, String)> = headers.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    let client = http_client(&plain)?;
    let config = StreamableHttpClientTransportConfig::with_uri(url.as_str());
    let challenge = match handler().0.serve(StreamableHttpClientTransport::with_client(client, config)).await {
        Ok(running) => {
            let _ = running.cancel().await;
            return Ok(format!("{} needs no sign-in: it answered without one", service.name));
        }
        Err(e) => e.auth_challenge().map(str::to_string).ok_or_else(|| anyhow!("{} did not answer: {e}", service.name))?,
    };
    let mut state = OAuthState::new(url.as_str(), Some(reqwest::Client::new()))
        .await
        .map_err(|e| anyhow!("{} names no sign-in web2 can follow: {e}", service.name))?;
    state
        .start_authorization(AuthorizationRequest::new(redirect_uri(port)).with_client_name("web2").with_challenge(challenge))
        .await
        .map_err(|e| anyhow!("{} would not start a sign-in: {e}", service.name))?;
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .with_context(|| format!("listen for the sign-in callback on 127.0.0.1:{port}"))?;
    show_sign_in(&service.name, &state.get_authorization_url().await.map_err(|e| anyhow!("{e}"))?);
    let params = await_callback(listener).await?;
    let code = params.get("code").ok_or_else(|| anyhow!("the callback carried no code"))?;
    let csrf = params.get("state").ok_or_else(|| anyhow!("the callback carried no state"))?;
    state
        .handle_callback_with_issuer(code, csrf, params.get("iss").map(String::as_str))
        .await
        .map_err(|e| anyhow!("{} did not hand over a token: {e}", service.name))?;
    let (client_id, token) = state.get_credentials().await.map_err(|e| anyhow!("{e}"))?;
    let received = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).ok();
    let stored = StoredCredentials::new(client_id, token, Vec::new(), received);
    creds.put(&service.name, json!({ "mcpOauth": serde_json::to_value(&stored)? }))?;
    let pool = Pool::new(Duration::from_secs(5));
    let (catalogue, _) = pool.catalogue(service, server, &[], creds).await.map_err(|e| anyhow!("{e}"))?;
    pool.drop_conn(&service.name).await;
    Ok(format!("{} is signed in; it offers {} commands", service.name, catalogue.commands.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_become_commands_and_only_a_read_only_hint_makes_a_read() {
        let cat = commands(&[
            json!({ "name": "search_issues", "description": "Search issues.\nMore.", "inputSchema": { "type": "object", "properties": { "q": { "type": "string" } } },
                    "annotations": { "readOnlyHint": true } }),
            json!({ "name": "createIssue", "title": "Create an issue", "inputSchema": null }),
            json!({ "name": "" }),
        ]);
        assert_eq!(cat.commands.len(), 2);
        let search = cat.find("search-issues").unwrap();
        assert!(!search.write);
        assert_eq!(search.summary, "Search issues.");
        assert!(matches!(&search.target, Target::Mcp { tool } if tool == "search_issues"));
        let create = cat.find("create_issue").unwrap();
        assert!(create.write);
        assert_eq!(create.summary, "Create an issue");
        assert_eq!(create.input["type"], "object");
    }
}
