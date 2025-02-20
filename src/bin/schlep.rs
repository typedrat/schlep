#![forbid(unsafe_code)]

use std::time::Duration;

use anyhow::Result;
use metrics_tracing_context::{MetricsLayer, TracingContextLayer};
use metrics_util::layers::Layer as _;
use mimalloc::MiMalloc;
use tracing_log::LogTracer;
use tracing_subscriber::{
    filter::{EnvFilter, LevelFilter},
    fmt::format::FmtSpan,
    prelude::*,
};

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

use schlep::{
    auth::AuthClient,
    config::Config,
    metrics::Metrics,
    sftp::SshServer,
    vfs::VfsSetBuilder,
};

#[tokio::main]
pub async fn main() -> Result<()> {
    LogTracer::init()?;

    let env_filter = EnvFilter::builder()
        .with_env_var("SCHLEP_LOG")
        .with_default_directive(LevelFilter::INFO.into())
        .from_env()?;
    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_span_events(FmtSpan::CLOSE)
        .with_filter(env_filter);
    let metrics_layer = MetricsLayer::new();
    let subscriber = tracing_subscriber::registry()
        .with(fmt_layer)
        .with(metrics_layer);
    tracing::subscriber::set_global_default(subscriber)?;

    let metrics_recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let metrics_handle = metrics_recorder.handle();

    {
        let metrics_handle = metrics_handle.clone();
        let upkeep_timeout = Duration::from_secs(5);

        tokio::spawn(async move {
            loop {
                tokio::time::sleep(upkeep_timeout).await;
                metrics_handle.run_upkeep();
            }
        });
    }

    let metrics_recorder = TracingContextLayer::all().layer(metrics_recorder);
    metrics::set_global_recorder(metrics_recorder)?;

    let config = Config::load()?;

    let redis_pool = if let Some(redis_config) = config.redis {
        Some(redis_config.get_pool()?)
    } else {
        None
    };
    let auth_client = AuthClient::new(config.auth.clone(), redis_pool.clone()).await?;
    let vfs_builder = VfsSetBuilder::from_config(config.fs)?;

    let metrics_server = Metrics::new(config.metrics.clone(), metrics_handle);
    let mut ssh_server = SshServer::new(config.sftp.clone(), auth_client, vfs_builder.build());

    let ssh = tokio::spawn(async move { ssh_server.run().await });
    let metrics = tokio::spawn(async move { metrics_server.run().await });

    tokio::select! {
        ssh = ssh => { ssh??; }
        metrics = metrics => { metrics??; }
    }

    Ok(())
}
