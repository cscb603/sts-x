/*
 * server/mod.rs
 * Project: sts-x
 * Description: MCP-compatible HTTP server for AI consumption
 *
 * Exposes the search engine as an MCP Tool endpoint via HTTP.
 * AI agents call POST /search with a JSON query and receive
 * the AI-optimized SearchResponse format.
 *
 * Future: full MCP protocol compliance with SSE streaming.
 */

use crate::search::SearchEngine;
use crate::types::SearchQuery;
use crate::types::format::AiSearchOutput;
use axum::{
    extract::State,
    routing::post,
    Json, Router,
};
use std::sync::Arc;
use std::net::SocketAddr;
use tokio::sync::Mutex;

/// Shared application state
pub struct AppState {
    pub engine: Mutex<SearchEngine>,
}

/// Start the MCP search server
pub async fn serve(engine: SearchEngine, host: &str, port: u16) -> anyhow::Result<()> {
    let state = Arc::new(AppState { engine: Mutex::new(engine) });

    let app = Router::new()
        .route("/search", post(handle_search))
        .route("/health", post(handle_health))
        .with_state(state);

    let addr: SocketAddr = format!("{}:{}", host, port).parse()?;
    tracing::info!("STS-X server listening on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

/// POST /search — Main search endpoint (AI-optimized JSON)
async fn handle_search(
    State(state): State<Arc<AppState>>,
    Json(query): Json<SearchQuery>,
) -> Json<AiSearchOutput> {
    match state.engine.lock().await.search(query) {
        Ok(resp) => Json(resp.into()),
        Err(e) => {
            tracing::error!("Search error: {:?}", e);
            Json(AiSearchOutput {
                query: String::new(),
                results: Vec::new(),
                total_hits: 0,
                search_time_ms: 0,
                multi_hop: None,
                _ai_instructions: "",
            })
        }
    }
}

/// GET /health — Health check
async fn handle_health() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "service": "sts-x"
    }))
}
