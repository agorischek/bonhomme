mod commands;
mod files;
mod queries;
pub(crate) mod session;
mod slice_audit;

use crate::agents;
use crate::api;
use crate::explorer;
use anyhow::{Context, Result};
use bonhomme_engine::Storage;
use clap::{Args, Parser, Subcommand, ValueEnum};
use commands::run_storage_command;
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
};
use uuid::Uuid;

#[derive(Parser)]
#[command(name = "bonhomme")]
#[command(
    about = "Semantic source control prototype for TypeScript/JavaScript, Go, Rust, Python, C#, and Elixir"
)]
struct Cli {
    /// Storage URL. Precedence: this flag / DATABASE_URL env > active session for repo-scoped
    /// commands > bonhomme.toml > the project-local Turso default.
    #[arg(long, env = "DATABASE_URL", global = true)]
    database_url: Option<String>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print repo-specific instructions for coding agents.
    Agents(agents::AgentsArgs),
    /// Serve the React demo/API used for development simulations.
    Server(ServerArgs),
    /// Serve one lightweight repo-scoped explorer instance from the core CLI.
    Explore(ExploreArgs),
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
    /// Coauth-session write-back: round-trip fidelity check and render-back into the working tree.
    Session {
        #[command(subcommand)]
        command: SessionCommand,
    },
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
struct ExploreArgs {
    /// Logical bonhomme repository name. Defaults to the discovered repo root directory name.
    #[arg(long)]
    repo: Option<String>,
    #[arg(long, default_value = "main")]
    branch: String,
    /// Bind address. Port 0 asks the OS for an available local port.
    #[arg(long, default_value = "127.0.0.1:0")]
    addr: SocketAddr,
    /// Open the explorer URL in the system browser after startup.
    #[arg(long, default_value_t = false)]
    open: bool,
}

#[derive(Args)]
struct InitArgs {
    /// Logical bonhomme repository name. Defaults to the active session repo, then repo root name.
    #[arg(long)]
    name: Option<String>,
}

#[derive(Args)]
struct ImportArgs {
    /// Logical bonhomme repository name. Defaults to the active session repo, then repo root name.
    #[arg(long)]
    repo: Option<String>,
    #[arg(long, default_value = "main")]
    branch: String,
    #[arg(long)]
    path: PathBuf,
    /// Replace the repository and rebuild every file instead of reconciling incrementally.
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
    /// Logical bonhomme repository name. Defaults to the active session repo, then repo root name.
    #[arg(long)]
    repo: Option<String>,
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
    /// Logical bonhomme repository name. Defaults to the active session repo, then repo root name.
    #[arg(long)]
    repo: Option<String>,
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
    /// Logical bonhomme repository name. Defaults to the active session repo, then repo root name.
    #[arg(long)]
    repo: Option<String>,
    #[arg(long, default_value = "main")]
    branch: String,
    #[arg(long)]
    symbol: Option<String>,
}

#[derive(Args)]
struct SliceApplyArgs {
    /// Logical bonhomme repository name. Defaults to the active session repo, then repo root name.
    #[arg(long)]
    repo: Option<String>,
    #[arg(long, default_value = "main")]
    branch: String,
    #[arg(long, default_value = "Apply edited semantic slice")]
    title: String,
    #[arg(long, default_value = "agent")]
    agent: String,
    /// Stored slice ID. Inferred when --modified is a full slice JSON object.
    #[arg(long)]
    slice_id: Option<Uuid>,
    #[arg(long)]
    original: Option<PathBuf>,
    /// Edited rendered file(s), or the full slice JSON returned by `slice create`.
    #[arg(long)]
    modified: PathBuf,
}

#[derive(Args)]
struct MergeArgs {
    /// Logical bonhomme repository name. Defaults to the active session repo, then repo root name.
    #[arg(long)]
    repo: Option<String>,
    #[arg(long)]
    source: String,
    #[arg(long, default_value = "main")]
    target: String,
}

#[derive(Args)]
struct BranchRefArgs {
    /// Logical bonhomme repository name. Defaults to the active session repo, then repo root name.
    #[arg(long)]
    repo: Option<String>,
    #[arg(long, default_value = "main")]
    branch: String,
}

#[derive(Args)]
struct RenderArgs {
    /// Logical bonhomme repository name. Defaults to the active session repo, then repo root name.
    #[arg(long)]
    repo: Option<String>,
    #[arg(long, default_value = "main")]
    branch: String,
    #[arg(long, default_value = "rendered")]
    out: PathBuf,
}

#[derive(Subcommand)]
enum SessionCommand {
    /// Import the working tree into a disposable local coauth session database.
    Start(SessionStartArgs),
    /// Import a tree and render it straight back, reporting any file that does not reproduce
    /// byte-for-byte. The write-back fidelity gate: exits non-zero if not diff-clean.
    Check(SessionCheckArgs),
    /// Open the lightweight explorer against the active coauth session.
    Review(SessionReviewArgs),
    /// Render a branch's files back into the working tree (write-back). Writing in place is gated
    /// by `git.write_back`; an explicit `--out` directory is always allowed.
    Land(SessionLandArgs),
}

#[derive(Args)]
struct SessionStartArgs {
    /// Directory to import. Defaults to the discovered project root.
    #[arg(long)]
    path: Option<PathBuf>,
    /// Logical bonhomme repository name. Defaults to the discovered repo root directory name.
    #[arg(long)]
    repo: Option<String>,
    #[arg(long, default_value = "main")]
    branch: String,
    /// Replace an existing local session for this repository.
    #[arg(long, default_value_t = false)]
    reset: bool,
    /// Session-start validation policy. Defaults to [validation].session_start in bonhomme.toml,
    /// then none.
    #[arg(long, value_enum)]
    validate: Option<SessionValidateArg>,
    /// Deprecated alias for --validate none.
    #[arg(long, default_value_t = false, conflicts_with = "validate")]
    no_validate: bool,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum SessionValidateArg {
    None,
    Toolchain,
}

impl From<SessionValidateArg> for crate::config::SessionValidationMode {
    fn from(value: SessionValidateArg) -> Self {
        match value {
            SessionValidateArg::None => Self::None,
            SessionValidateArg::Toolchain => Self::Toolchain,
        }
    }
}

#[derive(Args)]
struct SessionCheckArgs {
    /// Directory to check. Defaults to the discovered project root.
    #[arg(long)]
    path: Option<PathBuf>,
}

#[derive(Args)]
struct SessionReviewArgs {
    /// Logical bonhomme repository name. Defaults to the active session manifest.
    #[arg(long)]
    repo: Option<String>,
    /// Branch to review. Defaults to the active session manifest branch.
    #[arg(long)]
    branch: Option<String>,
    /// Bind address. Port 0 asks the OS for an available local port.
    #[arg(long, default_value = "127.0.0.1:0")]
    addr: SocketAddr,
    /// Open the explorer URL in the system browser after startup.
    #[arg(long, default_value_t = false)]
    open: bool,
}

#[derive(Args)]
struct SessionLandArgs {
    /// Logical bonhomme repository name. Defaults to the active session manifest.
    #[arg(long)]
    repo: Option<String>,
    /// Branch to land. Defaults to the active session manifest branch.
    #[arg(long)]
    branch: Option<String>,
    /// Write into this directory instead of the working tree (always allowed; non-destructive).
    #[arg(long)]
    out: Option<PathBuf>,
    /// Write into the working tree even when `git.write_back` is not enabled.
    #[arg(long, default_value_t = false)]
    force: bool,
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
    /// Logical bonhomme repository name. Defaults to the active session repo, then repo root name.
    #[arg(long)]
    repo: Option<String>,
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
    let (config, root) = crate::config::discover(&std::env::current_dir()?)?;
    let explicit_database_url = cli.database_url.clone();
    let active_session = if command_uses_active_session_database(&cli.command) {
        session::active_session(&root).await?
    } else {
        None
    };
    let database_url = resolve_database_url_for_command(
        explicit_database_url.clone(),
        &config,
        &root,
        active_session.as_ref(),
        &cli.command,
    );

    match cli.command {
        Command::Agents(args) => {
            agents::run(
                args,
                &config,
                &root,
                active_session.as_ref(),
                explicit_database_url.as_deref(),
                &database_url,
            )
            .await
        }
        Command::Server(args) => api::serve(Some(database_url), &config, &root, args.addr).await,
        Command::Explore(args) => {
            let repository_name = match args.repo {
                Some(repo) => repo,
                None => active_session
                    .as_ref()
                    .map(|session| session.repository.clone())
                    .map(Ok)
                    .unwrap_or_else(|| default_repository_name(&root))?,
            };
            let storage = Storage::connect(
                &database_url,
                crate::plugins::language_registry(&config, &root),
            )
            .await?;
            storage.migrate().await?;
            explorer::serve(
                storage,
                root.clone(),
                repository_name,
                args.branch,
                explorer::config_label(&root),
                explorer::database_label(&database_url),
                args.addr,
                args.open,
            )
            .await
        }
        Command::Session { command } => {
            session::run(command, &config, &root, explicit_database_url.as_deref()).await
        }
        command => {
            let storage = Storage::connect(
                &database_url,
                crate::plugins::language_registry(&config, &root),
            )
            .await?;
            storage.migrate().await?;
            run_storage_command(storage, command, &root).await
        }
    }
}

fn resolve_database_url_for_command(
    explicit_database_url: Option<String>,
    config: &crate::config::Config,
    root: &Path,
    active_session: Option<&session::ActiveSession>,
    command: &Command,
) -> String {
    if let Some(database_url) = explicit_database_url {
        return database_url;
    }
    if command_uses_active_session_database(command)
        && let Some(session) = active_session
    {
        return session.database_url.clone();
    }
    crate::config::resolve_database_url(None, config, root)
}

fn command_uses_active_session_database(command: &Command) -> bool {
    matches!(
        command,
        Command::Agents(_)
            | Command::Explore(_)
            | Command::Init(_)
            | Command::Import(_)
            | Command::Branch { .. }
            | Command::Task { .. }
            | Command::Slice { .. }
            | Command::Merge(_)
            | Command::Validate(_)
            | Command::Render(_)
            | Command::Query { .. }
    )
}

fn default_repository_name(root: &Path) -> Result<String> {
    root.file_name()
        .and_then(|name| name.to_str())
        .map(ToOwned::to_owned)
        .with_context(|| format!("could not infer repository name from {}", root.display()))
}
