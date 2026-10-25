//! An MCP server for web2's tests: four tools, over stdio, or over
//! streamable HTTP with `--http`, printing the port it listens on, and with
//! `--token <t>` refusing a request whose bearer is not `t`.

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
    if !args.iter().any(|a| a == "--http") {
        let running = Mock.serve(rmcp::transport::stdio()).await?;
        running.waiting().await?;
        return Ok(());
    }
    let token = args.iter().position(|a| a == "--token").and_then(|i| args.get(i + 1)).cloned();
    let service = StreamableHttpService::new(|| Ok(Mock), LocalSessionManager::default().into(), StreamableHttpServerConfig::default());
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
    let router = axum::Router::new().nest_service("/mcp", service).layer(guard);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    println!("listening {}", listener.local_addr()?.port());
    axum::serve(listener, router).await?;
    Ok(())
}
