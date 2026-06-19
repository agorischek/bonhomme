mod api;
mod cli;
mod core;
mod demo;
mod simulation;
mod storage;
mod ts;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    cli::run().await
}
