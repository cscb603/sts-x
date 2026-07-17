/*
 * main.rs
 * Project: sts-x
 * Description: CLI binary entry point
 */

use clap::Parser;
use sts_x::cli::{self, Cli};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing with sensible defaults
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "sts_x=info,tantivy=warn".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    cli::run(&cli).await?;

    Ok(())
}
