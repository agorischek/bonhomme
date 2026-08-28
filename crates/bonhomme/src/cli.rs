mod commands;
pub(crate) mod conflict_workspace;
mod files;
mod git_bridge;
mod queries;
pub(crate) mod session;
mod slice_audit;
mod slice_workspace;

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
    /// Import, export, or evaluate ordinary Git revisions without replacing Git as the ledger.
    Git {
        #[command(subcommand)]
        command: GitCommand,
    },
    /// Validate production configuration and verify operational integrity artifacts.
    Production {
        #[command(subcommand)]
        command: ProductionCommand,
    },
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
    Conflict {
        #[command(subcommand)]
        command: ConflictCommand,
    },
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
enum GitCommand {
    /// Import an arbitrary Git commit without checking it out in the user's working tree.
    Import(GitImportArgs),
    /// Export a Bonhomme branch as a standalone Git worktree and report its deterministic tree ID.
    Export(GitExportArgs),
    /// Compare Git's three-way merge with Bonhomme's semantic merge in read-only shadow mode.
    Shadow(GitShadowArgs),
    /// Run a reproducible directory-backed Git-vs-Bonhomme benchmark corpus.
    Benchmark(GitBenchmarkArgs),
}

#[derive(Subcommand)]
enum ProductionCommand {
    /// Fail closed when configuration is unsafe or incomplete for a network deployment.
    Check(ProductionCheckArgs),
    /// Verify sequence continuity and hashes in the configured append-only audit log.
    VerifyAudit(VerifyAuditArgs),
}

#[derive(Args)]
struct ProductionCheckArgs {
    /// Validate this manifest instead of the discovered bonhomme.toml.
    #[arg(long)]
    config: Option<PathBuf>,
    /// Intended production listener; non-loopback is recommended and requires authentication.
    #[arg(long, default_value = "0.0.0.0:3030")]
    addr: SocketAddr,
}

#[derive(Args)]
struct VerifyAuditArgs {
    /// Audit JSONL path. Defaults to <project-root>/.bonhomme/audit.jsonl.
    #[arg(long)]
    path: Option<PathBuf>,
}

#[derive(Args)]
struct GitImportArgs {
    /// Git working tree or bare repository containing the revision.
    #[arg(long, default_value = ".")]
    git_dir: PathBuf,
    /// Commit-ish to import. It is resolved and recorded as a full commit object ID.
    #[arg(long, default_value = "HEAD")]
    revision: String,
    /// Logical Bonhomme repository name.
    #[arg(long)]
    repo: Option<String>,
    /// Bonhomme branch receiving the imported snapshot.
    #[arg(long, default_value = "main")]
    branch: String,
    /// Replace the logical Bonhomme repository before importing.
    #[arg(long, default_value_t = false)]
    reset: bool,
    /// Skip configured language-toolchain validation after import.
    #[arg(long, default_value_t = false)]
    no_validate: bool,
}

#[derive(Args)]
struct GitExportArgs {
    /// Logical Bonhomme repository name.
    #[arg(long)]
    repo: Option<String>,
    #[arg(long, default_value = "main")]
    branch: String,
    /// New or empty directory that will become a standalone Git worktree.
    #[arg(long)]
    out: PathBuf,
}

#[derive(Args)]
struct GitShadowArgs {
    /// Git working tree or bare repository containing all three revisions.
    #[arg(long, default_value = ".")]
    git_dir: PathBuf,
    /// Explicit merge base commit-ish.
    #[arg(long)]
    base: String,
    /// Target-side commit-ish.
    #[arg(long)]
    ours: String,
    /// Source-side commit-ish.
    #[arg(long)]
    theirs: String,
    /// Run configured language toolchains against a safe semantic result.
    #[arg(long, default_value_t = false)]
    validate: bool,
}

#[derive(Args)]
struct GitBenchmarkArgs {
    /// Corpus whose immediate child directories contain case.toml and tree fixtures.
    #[arg(long, default_value = "benchmarks/semantic-merge/cases")]
    corpus: PathBuf,
    /// Run configured language toolchains against safe semantic results.
    #[arg(long, default_value_t = false)]
    validate: bool,
    /// Include wall-clock timings. Omit for byte-for-byte reproducible JSON reports.
    #[arg(long, default_value_t = false)]
    measure_time: bool,
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
    /// Write the checked-out slice to this directory instead of the default .bonhomme/slices path.
    #[arg(long, conflicts_with = "json")]
    out: Option<PathBuf>,
    /// Print the legacy JSON slice to stdout instead of checking out real files.
    #[arg(long, default_value_t = false)]
    json: bool,
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
    /// Checked-out slice directory. Defaults to the active slice from the last `slice create`.
    #[arg(long, conflicts_with = "modified")]
    dir: Option<PathBuf>,
    /// Legacy edited rendered file(s), or a full slice JSON object from `slice create --json`.
    #[arg(long)]
    modified: Option<PathBuf>,
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

#[derive(Subcommand)]
enum ConflictCommand {
    List(ConflictListArgs),
    Show(ConflictShowArgs),
    Checkout(ConflictCheckoutArgs),
    Resolve(ConflictResolveArgs),
    Abandon(ConflictAbandonArgs),
}

#[derive(Args)]
struct ConflictListArgs {
    /// Logical bonhomme repository name. Defaults to the active session repo, then repo root name.
    #[arg(long)]
    repo: Option<String>,
}

#[derive(Args)]
struct ConflictShowArgs {
    conflict_id: Uuid,
    #[arg(long, default_value_t = false)]
    json: bool,
}

#[derive(Args)]
struct ConflictCheckoutArgs {
    conflict_id: Uuid,
    /// Write the conflict workspace to this directory instead of .bonhomme/conflicts/<id>.
    #[arg(long)]
    out: Option<PathBuf>,
}

#[derive(Args)]
struct ConflictResolveArgs {
    conflict_id: Uuid,
    /// Conflict workspace directory or its resolved/ subdirectory. Defaults to the active checkout.
    #[arg(long)]
    dir: Option<PathBuf>,
    #[arg(long, default_value = "agent")]
    agent: String,
    #[arg(long, default_value_t = false)]
    accept_drop: bool,
}

#[derive(Args)]
struct ConflictAbandonArgs {
    conflict_id: Uuid,
    #[arg(long)]
    reason: Option<String>,
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
    let audit = match cli.command.mutation_audit_action() {
        Some(action) => Some(crate::security::CliMutationAudit::begin(&root, action).await?),
        None => None,
    };

    let result: Result<()> = async {
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
            Command::Server(args) => {
                api::serve(Some(database_url), &config, &root, args.addr).await
            }
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
            Command::Git {
                command: GitCommand::Shadow(args),
            } => {
                let plugin = crate::plugins::language_registry(&config, &root);
                git_bridge::run_shadow(plugin.as_ref(), args).await
            }
            Command::Git {
                command: GitCommand::Benchmark(args),
            } => {
                let plugin = crate::plugins::language_registry(&config, &root);
                git_bridge::run_benchmark(plugin.as_ref(), args, &root).await
            }
            Command::Production { command } => match command {
                ProductionCommand::Check(args) => {
                    let checked_config = match args.config {
                        Some(path) => crate::config::load(&path)?,
                        None => config,
                    };
                    let checked_database_url = crate::config::resolve_database_url(
                        explicit_database_url.clone(),
                        &checked_config,
                        &root,
                    );
                    crate::operations::check_production(
                        &checked_config,
                        &checked_database_url,
                        args.addr,
                    )
                }
                ProductionCommand::VerifyAudit(args) => {
                    let path = args
                        .path
                        .unwrap_or_else(|| root.join(".bonhomme").join("audit.jsonl"));
                    crate::operations::verify_audit(&path).await
                }
            },
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
    .await;

    if let Some(audit) = audit {
        let succeeded = result.is_ok();
        if let Err(audit_error) = audit.finish(succeeded).await {
            return match result {
                Ok(()) => Err(audit_error.context(
                    "CLI mutation completed but its durable audit outcome could not be written; inspect state before retrying",
                )),
                Err(command_error) => Err(command_error.context(format!(
                    "the CLI mutation also failed to write its audit outcome: {audit_error}"
                ))),
            };
        }
    }
    result
}

impl Command {
    /// Return a static audit action for commands that persist Bonhomme state or write a durable
    /// user-visible artifact. Arguments are deliberately excluded so secrets and payload bodies can
    /// never enter the audit log through command classification.
    fn mutation_audit_action(&self) -> Option<&'static str> {
        match self {
            Self::Init(_) => Some("init"),
            Self::Import(_) => Some("import"),
            Self::Git {
                command: GitCommand::Import(_),
            } => Some("git/import"),
            Self::Git {
                command: GitCommand::Export(_),
            } => Some("git/export"),
            Self::Branch {
                command: BranchCommand::Create(_),
            } => Some("branch/create"),
            Self::Task {
                command: TaskCommand::Create(_),
            } => Some("task/create"),
            Self::Slice {
                command: SliceCommand::Create(_),
            } => Some("slice/create"),
            Self::Slice {
                command: SliceCommand::Apply(_),
            } => Some("slice/apply"),
            Self::Merge(_) => Some("merge"),
            Self::Conflict {
                command: ConflictCommand::Checkout(_),
            } => Some("conflict/checkout"),
            Self::Conflict {
                command: ConflictCommand::Resolve(_),
            } => Some("conflict/resolve"),
            Self::Conflict {
                command: ConflictCommand::Abandon(_),
            } => Some("conflict/abandon"),
            Self::Render(_) => Some("render"),
            Self::Session {
                command: SessionCommand::Start(_),
            } => Some("session/start"),
            Self::Session {
                command: SessionCommand::Land(_),
            } => Some("session/land"),
            Self::Simulate(_) => Some("simulate"),
            Self::Demo {
                command: DemoCommand::Reset,
            } => Some("demo/reset"),
            Self::Demo {
                command: DemoCommand::Spawn(_),
            } => Some("demo/spawn"),
            Self::Demo {
                command: DemoCommand::MergeAll,
            } => Some("demo/merge-all"),
            Self::Agents(_)
            | Self::Server(_)
            | Self::Explore(_)
            | Self::Git {
                command: GitCommand::Shadow(_) | GitCommand::Benchmark(_),
            }
            | Self::Production { .. }
            | Self::Conflict {
                command: ConflictCommand::List(_) | ConflictCommand::Show(_),
            }
            | Self::Validate(_)
            | Self::Session {
                command: SessionCommand::Check(_) | SessionCommand::Review(_),
            }
            | Self::Query { .. } => None,
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
            | Command::Git {
                command: GitCommand::Import(_) | GitCommand::Export(_),
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn audit_action(arguments: &[&str]) -> Option<&'static str> {
        Cli::try_parse_from(arguments)
            .unwrap()
            .command
            .mutation_audit_action()
    }

    #[test]
    fn persistent_cli_commands_have_static_audit_classifications() {
        assert_eq!(
            audit_action(&["bonhomme", "init", "--name", "repo"]),
            Some("init")
        );
        assert_eq!(
            audit_action(&[
                "bonhomme", "git", "export", "--repo", "repo", "--out", "exported"
            ]),
            Some("git/export")
        );
        assert_eq!(
            audit_action(&[
                "bonhomme",
                "slice",
                "apply",
                "--repo",
                "repo",
                "--modified",
                "edited.json",
                "--original",
                "base.json"
            ]),
            Some("slice/apply")
        );
        assert_eq!(
            audit_action(&[
                "bonhomme",
                "conflict",
                "resolve",
                "00000000-0000-0000-0000-000000000001"
            ]),
            Some("conflict/resolve")
        );
        assert_eq!(
            audit_action(&["bonhomme", "session", "land", "--out", "rendered"]),
            Some("session/land")
        );
        assert_eq!(
            audit_action(&["bonhomme", "demo", "reset"]),
            Some("demo/reset")
        );
    }

    #[test]
    fn read_only_and_service_cli_commands_are_not_audited_as_mutations() {
        assert_eq!(
            audit_action(&["bonhomme", "validate", "--repo", "repo"]),
            None
        );
        assert_eq!(
            audit_action(&[
                "bonhomme",
                "query",
                "find-symbol",
                "--repo",
                "repo",
                "--name",
                "OrderService"
            ]),
            None
        );
        assert_eq!(
            audit_action(&[
                "bonhomme", "git", "shadow", "--base", "base", "--ours", "ours", "--theirs",
                "theirs"
            ]),
            None
        );
        assert_eq!(
            audit_action(&["bonhomme", "conflict", "list", "--repo", "repo"]),
            None
        );
        assert_eq!(audit_action(&["bonhomme", "session", "check"]), None);
        // Long-running services audit their mutations at the authenticated request boundary instead
        // of emitting a misleading CLI outcome only when the process eventually exits.
        assert_eq!(audit_action(&["bonhomme", "server"]), None);
        assert_eq!(audit_action(&["bonhomme", "explore"]), None);
    }

    #[test]
    fn every_cli_leaf_has_an_explicit_audit_classification() {
        let cases: Vec<(Vec<&str>, Option<&str>)> = vec![
            (vec!["bonhomme", "agents"], None),
            (vec!["bonhomme", "server"], None),
            (vec!["bonhomme", "explore"], None),
            (vec!["bonhomme", "init"], Some("init")),
            (vec!["bonhomme", "import", "--path", "."], Some("import")),
            (vec!["bonhomme", "git", "import"], Some("git/import")),
            (
                vec!["bonhomme", "git", "export", "--out", "exported"],
                Some("git/export"),
            ),
            (
                vec![
                    "bonhomme", "git", "shadow", "--base", "base", "--ours", "ours", "--theirs",
                    "theirs",
                ],
                None,
            ),
            (vec!["bonhomme", "git", "benchmark"], None),
            (vec!["bonhomme", "production", "check"], None),
            (vec!["bonhomme", "production", "verify-audit"], None),
            (
                vec!["bonhomme", "branch", "create", "--name", "feature"],
                Some("branch/create"),
            ),
            (
                vec!["bonhomme", "task", "create", "--title", "task"],
                Some("task/create"),
            ),
            (vec!["bonhomme", "slice", "create"], Some("slice/create")),
            (
                vec![
                    "bonhomme",
                    "slice",
                    "apply",
                    "--modified",
                    "edited.json",
                    "--original",
                    "base.json",
                ],
                Some("slice/apply"),
            ),
            (
                vec!["bonhomme", "merge", "--source", "feature"],
                Some("merge"),
            ),
            (vec!["bonhomme", "conflict", "list"], None),
            (
                vec![
                    "bonhomme",
                    "conflict",
                    "show",
                    "00000000-0000-0000-0000-000000000001",
                ],
                None,
            ),
            (
                vec![
                    "bonhomme",
                    "conflict",
                    "checkout",
                    "00000000-0000-0000-0000-000000000001",
                ],
                Some("conflict/checkout"),
            ),
            (
                vec![
                    "bonhomme",
                    "conflict",
                    "resolve",
                    "00000000-0000-0000-0000-000000000001",
                ],
                Some("conflict/resolve"),
            ),
            (
                vec![
                    "bonhomme",
                    "conflict",
                    "abandon",
                    "00000000-0000-0000-0000-000000000001",
                ],
                Some("conflict/abandon"),
            ),
            (vec!["bonhomme", "validate"], None),
            (vec!["bonhomme", "render"], Some("render")),
            (vec!["bonhomme", "session", "start"], Some("session/start")),
            (vec!["bonhomme", "session", "check"], None),
            (vec!["bonhomme", "session", "review"], None),
            (
                vec!["bonhomme", "session", "land", "--out", "rendered"],
                Some("session/land"),
            ),
            (vec!["bonhomme", "simulate"], Some("simulate")),
            (
                vec!["bonhomme", "query", "find-symbol", "--name", "x"],
                None,
            ),
            (
                vec!["bonhomme", "query", "find-references", "--name", "x"],
                None,
            ),
            (
                vec!["bonhomme", "query", "find-callers", "--name", "x"],
                None,
            ),
            (
                vec!["bonhomme", "query", "find-callees", "--name", "x"],
                None,
            ),
            (
                vec!["bonhomme", "query", "find-dependencies", "--name", "x"],
                None,
            ),
            (
                vec!["bonhomme", "query", "find-dependents", "--name", "x"],
                None,
            ),
            (vec!["bonhomme", "demo", "reset"], Some("demo/reset")),
            (vec!["bonhomme", "demo", "spawn"], Some("demo/spawn")),
            (
                vec!["bonhomme", "demo", "merge-all"],
                Some("demo/merge-all"),
            ),
        ];

        assert_eq!(cases.len(), 37);
        for (arguments, expected) in cases {
            assert_eq!(
                audit_action(&arguments),
                expected,
                "arguments: {arguments:?}"
            );
        }
    }
}
