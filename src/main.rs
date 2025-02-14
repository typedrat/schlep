use anyhow::Result;
use schlep::main as lib_main;

#[tokio::main]
pub async fn main() -> Result<()> {
    lib_main().await
}
