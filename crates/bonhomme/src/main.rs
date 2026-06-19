mod api;
mod cli;
mod demo;
mod simulation;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    cli::run().await
}
