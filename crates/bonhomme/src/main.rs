mod api;
mod cli;
mod demo;
mod plugins;
mod simulation;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    cli::run().await
}
