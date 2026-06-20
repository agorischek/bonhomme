mod commands;
mod files;
mod queries;
mod slice_audit;

use crate::api;
use crate::demo::DEMO_REPOSITORY;
use anyhow::Result;
use bonhomme_engine::{DEFAULT_DATABASE_URL, Storage};
use clap::{Args, Parser, Subcommand};
use commands::run_storage_command;
use std::{net::SocketAddr, path::PathBuf};
use uuid::Uuid;

#[derive(Parser)]
#[command(name = "bonhomme")]
#[command(about = "Semantic source control prototype for TypeScript")]
struct Cli {
    #[arg(long, env = "DATABASE_URL", global = true, default_value = DEFAULT_DATABASE_URL)]
    database_url: String,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Server(ServerArgs),
    Init(InitArgs),
    Import(ImportArgs),
    Branch {
        #[command(subcommand)]
        command: BranchCommand,
    },
    Task {
        #[command(subcommand)]
        command: TaskCommand,
    },
    Slice {
        #[command(subcommand)]
        command: SliceCommand,
    },
    Merge(MergeArgs),
    Validate(BranchRefArgs),
    Render(RenderArgs),
    Simulate(SimulateArgs),
    Query {
        #[command(subcommand)]
        command: QueryCommand,
    },
    Demo {
        #[command(subcommand)]
        command: DemoCommand,
    },
}

#[derive(Args)]
struct ServerArgs {
    #[arg(long, default_value = "127.0.0.1:3030")]
    addr: SocketAddr,
}

#[derive(Args)]
struct InitArgs {
    #[arg(long, default_value = DEMO_REPOSITORY)]
    name: String,
}

#[derive(Args)]
struct ImportArgs {
    #[arg(long, default_value = DEMO_REPOSITORY)]
    repo: String,
    #[arg(long, default_value = "main")]
    branch: String,
    #[arg(long)]
    path: PathBuf,
    #[arg(long, default_value_t = false)]
    reset: bool,
    #[arg(long, default_value_t = false)]
    no_validate: bool,
}

#[derive(Subcommand)]
enum BranchCommand {
    Create(BranchCreateArgs),
}

#[derive(Args)]
struct BranchCreateArgs {
    #[arg(long, default_value = DEMO_REPOSITORY)]
    repo: String,
    #[arg(long)]
    name: String,
    #[arg(long, default_value = "main")]
    base: String,
}

#[derive(Subcommand)]
enum TaskCommand {
    Create(TaskCreateArgs),
}

#[derive(Args)]
struct TaskCreateArgs {
    #[arg(long, default_value = DEMO_REPOSITORY)]
    repo: String,
    #[arg(long)]
    title: String,
}

#[derive(Subcommand)]
enum SliceCommand {
    Create(SliceCreateArgs),
    Apply(SliceApplyArgs),
}

#[derive(Args)]
struct SliceCreateArgs {
    #[arg(long, default_value = DEMO_REPOSITORY)]
    repo: String,
    #[arg(long, default_value = "main")]
    branch: String,
    #[arg(long)]
    symbol: Option<String>,
}

#[derive(Args)]
struct SliceApplyArgs {
    #[arg(long, default_value = DEMO_REPOSITORY)]
    repo: String,
    #[arg(long, default_value = "main")]
    branch: String,
    #[arg(long, default_value = "Apply edited TypeScript slice")]
    title: String,
    #[arg(long, default_value = "agent")]
    agent: String,
    #[arg(long)]
    slice_id: Option<Uuid>,
    #[arg(long)]
    original: Option<PathBuf>,
    #[arg(long)]
    modified: PathBuf,
}

#[derive(Args)]
struct MergeArgs {
    #[arg(long, default_value = DEMO_REPOSITORY)]
    repo: String,
    #[arg(long)]
    source: String,
    #[arg(long, default_value = "main")]
    target: String,
}

#[derive(Args)]
struct BranchRefArgs {
    #[arg(long, default_value = DEMO_REPOSITORY)]
    repo: String,
    #[arg(long, default_value = "main")]
    branch: String,
}

#[derive(Args)]
struct RenderArgs {
    #[arg(long, default_value = DEMO_REPOSITORY)]
    repo: String,
    #[arg(long, default_value = "main")]
    branch: String,
    #[arg(long, default_value = "rendered")]
    out: PathBuf,
}

#[derive(Args)]
struct SimulateArgs {
    #[arg(long, default_value_t = 128)]
    agents: usize,
    #[arg(long, default_value_t = false)]
    conflicts: bool,
    #[arg(long, default_value = "typescript")]
    language: String,
}

#[derive(Subcommand)]
// CLI subcommand names derive from these variant identifiers, so the shared `Find` prefix is
// intentional and renaming would break the command surface.
#[allow(clippy::enum_variant_names)]
enum QueryCommand {
    FindSymbol(FindSymbolArgs),
    FindReferences(FindSymbolArgs),
    FindCallers(FindSymbolArgs),
    FindCallees(FindSymbolArgs),
    FindDependencies(FindSymbolArgs),
    FindDependents(FindSymbolArgs),
}

#[derive(Args)]
struct FindSymbolArgs {
    #[arg(long, default_value = DEMO_REPOSITORY)]
    repo: String,
    #[arg(long, default_value = "main")]
    branch: String,
    #[arg(long)]
    name: String,
}

#[derive(Subcommand)]
enum DemoCommand {
    Reset,
    Spawn(DemoSpawnArgs),
    MergeAll,
}

#[derive(Args)]
struct DemoSpawnArgs {
    #[arg(long, default_value_t = 24)]
    count: usize,
    #[arg(long, default_value_t = false)]
    conflicts: bool,
}

pub async fn run() -> Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "bonhomme=info,tower_http=info".into()),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Command::Server(args) => api::serve(Some(cli.database_url), args.addr).await,
        command => {
            let storage =
                Storage::connect(&cli.database_url, crate::plugins::language_registry()).await?;
            storage.migrate().await?;
            run_storage_command(storage, command).await
        }
    }
}
