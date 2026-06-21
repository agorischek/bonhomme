mod agents;
mod api;
mod cli;
mod config;
mod demo;
mod explorer;
mod format;
mod plugins;
mod simulation;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    cli::run().await
}
