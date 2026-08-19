use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use duckdb::Connection;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerInfo};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ServerHandler, ServiceExt, tool, tool_handler, tool_router};
use serde::Deserialize;

use crate::cli::{Format, SearchArgs};
use crate::query;
use crate::search;

/// Runs the MCP server on stdio until the client disconnects.
pub fn run_stdio(db: &Path) -> Result<()> {
    let server = McpServer::new(query::open_read_only(db)?);
    // One client, one request at a time: no worker threads needed.
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async {
            let service = server
                .serve(rmcp::transport::stdio())
                .await
                .context("failed to start MCP server")?;
            service.waiting().await?;
            anyhow::Ok(())
        })
}

/// Runs the MCP server over Streamable HTTP at `/mcp` on `addr`.
pub fn run_http(db: &Path, addr: SocketAddr) -> Result<()> {
    let server = McpServer::new(query::open_read_only(db)?);
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async {
            let listener = tokio::net::TcpListener::bind(addr)
                .await
                .with_context(|| format!("failed to bind {addr}"))?;
            eprintln!(
                "MCP server listening on http://{}/mcp",
                listener.local_addr()?
            );
            axum::serve(listener, http_router(server)).await?;
            anyhow::Ok(())
        })
}

/// Builds the HTTP app serving MCP at `/mcp`.
///
/// Sessions are disabled on purpose: every tool is pure request/response, so
/// there is no per-client state to keep, and MCP drops sessions entirely in
/// protocol version 2026-07-28. rmcp's default `Host` allowlist (loopback
/// only) is kept as the guard against DNS rebinding.
pub fn http_router(server: McpServer) -> axum::Router {
    let service: StreamableHttpService<McpServer, LocalSessionManager> = StreamableHttpService::new(
        move || Ok(server.clone()),
        Default::default(),
        StreamableHttpServerConfig::default()
            .with_legacy_session_mode(false)
            .with_json_response(true),
    );
    axum::Router::new().nest_service("/mcp", service)
}

#[derive(Clone)]
pub struct McpServer {
    // ServerHandler requires Sync and duckdb::Connection is not. One shared
    // read-only connection behind a Mutex serializes tool calls, which at
    // archive scale (milliseconds per query) costs nothing even over HTTP.
    conn: Arc<Mutex<Connection>>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct SearchTweetsParams {
    /// Keywords, ANDed, matched case-insensitively as substrings
    /// (works for Japanese as well as space-delimited languages)
    #[serde(default)]
    keywords: Vec<String>,
    /// Only tweets of the account with this username
    account: Option<String>,
    /// Only tweets on or after this date (YYYY-MM-DD)
    since: Option<String>,
    /// Only tweets on or before this date (YYYY-MM-DD)
    until: Option<String>,
    /// Only tweets mentioning this screen name
    mentions: Option<String>,
    /// Only replies to this screen name
    reply_to: Option<String>,
    /// Only tweets with attached media
    #[serde(default)]
    has_media: bool,
    /// Only tweets containing a link
    #[serde(default)]
    has_url: bool,
    /// Exclude retweets
    #[serde(default)]
    no_retweets: bool,
    /// Include deleted tweets
    #[serde(default)]
    include_deleted: bool,
    /// Show only deleted tweets
    #[serde(default)]
    only_deleted: bool,
    /// Sort oldest first (default newest first)
    #[serde(default)]
    oldest_first: bool,
    /// Maximum rows to return (default 20)
    limit: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct SearchLikesParams {
    /// Keywords, ANDed, matched case-insensitively as substrings against the
    /// liked tweet's text
    #[serde(default)]
    keywords: Vec<String>,
    /// Only likes of the account with this username
    account: Option<String>,
    /// Sort oldest first (default newest first)
    #[serde(default)]
    oldest_first: bool,
    /// Maximum rows to return (default 20)
    limit: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct QueryParams {
    /// SQL to run (SELECT form)
    sql: String,
    /// Maximum rows to return (default 100, 0 disables the cap)
    limit: Option<usize>,
}

#[tool_router]
impl McpServer {
    pub fn new(conn: Connection) -> Self {
        Self {
            conn: Arc::new(Mutex::new(conn)),
        }
    }

    /// Runs `f` against the connection and turns its output into a tool
    /// result. Failures (e.g. bad SQL) become tool-level errors so the
    /// calling model sees the message instead of an opaque protocol error.
    fn call(&self, f: impl FnOnce(&Connection, &mut Vec<u8>) -> Result<()>) -> CallToolResult {
        let conn = self.conn.lock().unwrap();
        let mut out = Vec::new();
        match f(&conn, &mut out) {
            Ok(()) => CallToolResult::success(vec![ContentBlock::text(
                String::from_utf8_lossy(&out).into_owned(),
            )]),
            Err(e) => CallToolResult::error(vec![ContentBlock::text(format!("{e:#}"))]),
        }
    }

    #[tool(
        description = "Search archived tweets by keyword and filters. Deleted tweets are excluded unless requested. Returns one JSON object per line."
    )]
    fn search_tweets(&self, Parameters(p): Parameters<SearchTweetsParams>) -> CallToolResult {
        let args = SearchArgs {
            keywords: p.keywords,
            account: p.account,
            likes: false,
            since: p.since,
            until: p.until,
            mentions: p.mentions,
            reply_to: p.reply_to,
            has_media: p.has_media,
            has_url: p.has_url,
            no_retweets: p.no_retweets,
            include_deleted: p.include_deleted,
            only_deleted: p.only_deleted,
            oldest_first: p.oldest_first,
            limit: p.limit.unwrap_or(20),
            format: Format::Jsonl,
        };
        self.call(|conn, out| search::run(conn, out, &args))
    }

    #[tool(
        description = "Search liked tweets by keyword. Likes carry no timestamp; ordering approximates recency via tweet ID. Returns one JSON object per line."
    )]
    fn search_likes(&self, Parameters(p): Parameters<SearchLikesParams>) -> CallToolResult {
        let args = SearchArgs {
            keywords: p.keywords,
            account: p.account,
            likes: true,
            since: None,
            until: None,
            mentions: None,
            reply_to: None,
            has_media: false,
            has_url: false,
            no_retweets: false,
            include_deleted: false,
            only_deleted: false,
            oldest_first: p.oldest_first,
            limit: p.limit.unwrap_or(20),
            format: Format::Jsonl,
        };
        self.call(|conn, out| search::run(conn, out, &args))
    }

    #[tool(
        description = "Run a read-only SQL query (DuckDB syntax) against the archive database. Call schema first to see the available tables and columns. Returns one JSON object per line."
    )]
    fn query(&self, Parameters(p): Parameters<QueryParams>) -> CallToolResult {
        let limit = p.limit.unwrap_or(100);
        self.call(|conn, out| {
            query::run_query(
                conn,
                out,
                Format::Jsonl,
                &p.sql,
                &[],
                (limit > 0).then_some(limit),
            )
        })
    }

    #[tool(
        description = "Show the tables, columns and row counts available for querying. Returns one JSON object per line."
    )]
    fn schema(&self) -> CallToolResult {
        self.call(|conn, out| query::print_schema(conn, out, Format::Jsonl))
    }
}

#[tool_handler]
impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                env!("CARGO_PKG_NAME"),
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(
                "Searches X (Twitter) archives imported into a local DuckDB database. \
             The database is opened read-only. Use search_tweets or search_likes \
             for keyword search; for anything else, call schema to see the tables \
             (accounts, tweets, likes, followers, following) and run SQL with query.",
            )
    }
}
