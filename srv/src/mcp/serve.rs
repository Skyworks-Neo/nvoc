//! Streamable-HTTP wiring for the MCP endpoint (axum + rmcp).

use super::handler::NvocMcp;
use crate::http::ServerState;
use crate::runtime::lock;
use axum::response::IntoResponse;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use std::sync::Arc;

/// Bind the configured loopback port and serve until it stops. Returns on bind
/// failure (the caller retries) or when the server exits.
pub async fn serve_once(state: Arc<ServerState>) {
    let addr = format!("127.0.0.1:{}", lock(&state.config).mcp.port);
    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            log::error!("mcp: failed to bind {addr}: {e}");
            return;
        }
    };
    serve_on(listener, state).await;
}

/// Serve the MCP endpoint on an already-bound listener. Split out so tests can
/// bind an ephemeral port.
pub async fn serve_on(listener: tokio::net::TcpListener, state: Arc<ServerState>) {
    let token = lock(&state.config).mcp.token.clone();
    if let Ok(addr) = listener.local_addr() {
        log::info!("mcp: control-plane endpoint listening on {addr}");
    }

    // One service instance per MCP session; each holds only a shared handle to
    // srv state, so construction is cheap (and the schema-probe instances that
    // rmcp builds never open an nvoc session — that happens lazily on the first
    // control tool call).
    let service = StreamableHttpService::new(
        {
            let state = state.clone();
            move || -> Result<NvocMcp, std::io::Error> { Ok(NvocMcp::new(state.clone())) }
        },
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );

    let router = axum::Router::new().fallback_service(service);
    let router = match token {
        Some(token) => router.layer(axum::middleware::from_fn(move |req, next| {
            let token = token.clone();
            async move { require_bearer(req, next, token).await }
        })),
        None => router,
    };

    if let Err(e) = axum::serve(listener, router).await {
        log::error!("mcp: serve error: {e}");
    }
}

/// Constant-time-ish bearer check for the optional `[mcp] token`. Loopback-only
/// bind makes this defense in depth, not the primary trust edge.
async fn require_bearer(
    req: axum::extract::Request,
    next: axum::middleware::Next,
    token: String,
) -> axum::response::Response {
    let expected = format!("Bearer {token}");
    let ok = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.trim() == expected);
    if ok {
        next.run(req).await
    } else {
        (
            axum::http::StatusCode::UNAUTHORIZED,
            "missing or invalid bearer token",
        )
            .into_response()
    }
}
