use std::sync::Arc;

use axum::{extract::State, response::IntoResponse, routing, Router};
use http::{HeaderMap, StatusCode};
use metrics_exporter_prometheus::PrometheusHandle;
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

    #[serde_inline_default(true)]
    pub enable_metrics_export: bool,
}

pub struct Metrics {
    config: Arc<Config>,
    handle: Arc<PrometheusHandle>,
}

impl Metrics {
    pub fn new(config: Config, handle: PrometheusHandle) -> Self {
        Self {
            config: Arc::new(config),
            handle: Arc::new(handle),
        }
    }

    pub async fn run(&self) -> anyhow::Result<()> {
        let app = Router::new()
            .route("/healthz", routing::get(Self::healthz_handler))
            .route(
                "/metrics",
                routing::get({
                    let handle = self.handle.clone();
                    move |config| Self::prometheus_handler(config, handle)
                }),
            )
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

    async fn prometheus_handler(
        State(config): State<Arc<Config>>,
        handle: Arc<PrometheusHandle>,
    ) -> impl IntoResponse {
        if config.enable_metrics_export {
            let metrics = handle.render();
            (StatusCode::OK, metrics)
        } else {
            (StatusCode::NOT_FOUND, "".to_string())
        }
    }
}
