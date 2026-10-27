use anyhow::Result;
use tracing_subscriber::{fmt, EnvFilter};

pub fn init(level: &str) -> Result<()> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level));
    let json = std::env::var("ONEAPI_LOG_JSON")
        .map(|value| matches!(value.to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false);

    let builder = fmt().with_env_filter(filter).with_target(false);
    if json {
        builder
            .json()
            .try_init()
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    } else {
        builder
            .try_init()
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    }

    Ok(())
}
