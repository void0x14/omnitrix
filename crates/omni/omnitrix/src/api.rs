#![allow(dead_code)]

use axum::{
    Router,
    extract::ws::WebSocketUpgrade,
    response::Json,
    routing::get,
};
use tokio::sync::broadcast;

use omni_webui::ws_handlers;

#[derive(Clone)]
pub struct ControlPlaneApi {
    pub addr: String,
}

impl ControlPlaneApi {
    pub fn new(addr: &str) -> Self {
        Self {
            addr: addr.to_string(),
        }
    }

    pub async fn serve(&self) -> anyhow::Result<()> {
        let (tx, _) = broadcast::channel::<String>(256);

        let app = Router::new()
            .route("/health", get(health_handler))
            .route("/agents", get(agents_handler))
            .route(
                "/ws/events",
                get(move |ws: WebSocketUpgrade| ws_handlers::ws_handler(ws, tx.clone())),
            );

        let listener = tokio::net::TcpListener::bind(&self.addr).await?;
        axum::serve(listener, app).await?;
        Ok(())
    }
}

async fn health_handler() -> Json<serde_json::Value> {
    Json(serde_json::json!({"status": "ok"}))
}

async fn agents_handler() -> Json<serde_json::Value> {
    Json(serde_json::json!({"agents": []}))
}
