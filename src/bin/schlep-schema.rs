use anyhow::Result;
use schemars::schema_for;
use schlep::config::Config;

pub fn main() -> Result<()> {
    let schema = schema_for!(Config);

    println!("{}", serde_json::to_string_pretty(&schema)?);

    Ok(())
}
