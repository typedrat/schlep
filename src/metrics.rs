use std::sync::Arc;

use axum::{extract::State, response::IntoResponse, routing, Router};
use http::{HeaderMap, StatusCode};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_inline_default::serde_inline_default;
use tokio::net::TcpListener;

use crate::version::VERSION_INFO;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde_inline_default]
pub struct Config {
    pub address: String,
    pub port: u16,

    #[serde_inline_default(true)]
    pub enable_health_check: bool,
}

pub struct Metrics {
    config: Arc<Config>,
}

impl Metrics {
    pub fn new(config: Config) -> Self {
        Self {
            config: Arc::new(config),
        }
    }

    pub async fn run(&self) -> anyhow::Result<()> {
        let app = Router::new()
            .route("/healthz", routing::get(Self::healthz_handler))
            .with_state(self.config.clone());

        let listener = TcpListener::bind((self.config.address.clone(), self.config.port)).await?;
        axum::serve(listener, app).await?;

        Ok(())
    }

    async fn healthz_handler(State(config): State<Arc<Config>>) -> impl IntoResponse {
        if config.enable_health_check {
            (StatusCode::OK, VERSION_INFO.as_headers())
        } else {
            (StatusCode::NOT_FOUND, HeaderMap::default())
        }
    }
}
