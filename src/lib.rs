#![forbid(unsafe_code)]
use anyhow::Result;
use metrics::Metrics;
use mimalloc::MiMalloc;
use tracing_subscriber::{self, fmt::format::FmtSpan};

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

pub mod auth;
pub mod config;
pub mod metrics;
mod redis;
pub mod sftp;
pub mod version;
pub mod vfs;

use auth::AuthClient;
use config::Config;
use sftp::SshServer;
use vfs::VfsSetBuilder;

pub async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_span_events(FmtSpan::CLOSE)
        .init();

    let config = Config::load()?;

    let redis_pool = if let Some(redis_config) = config.redis {
        Some(redis_config.get_pool()?)
    } else {
        None
    };
    let auth_client = AuthClient::new(config.auth.clone(), redis_pool.clone()).await?;
    let vfs_builder = VfsSetBuilder::from_config(config.fs)?;

    let metrics_server = Metrics::new(config.metrics.clone());
    let mut ssh_server = SshServer::new(config.sftp.clone(), auth_client, vfs_builder.build());

    let ssh = tokio::spawn(async move { ssh_server.run().await });
    let metrics = tokio::spawn(async move { metrics_server.run().await });

    tokio::select! {
        ssh = ssh => { ssh??; }
        metrics = metrics => { metrics??; }
    }

    Ok(())
}
