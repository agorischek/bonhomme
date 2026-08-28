mod agents;
mod api;
mod cli;
mod config;
mod demo;
mod explorer;
mod format;
mod operations;
mod plugins;
mod security;
mod simulation;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    cli::run().await
}
