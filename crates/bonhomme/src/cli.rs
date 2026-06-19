use crate::api;
use crate::core::{SemanticGraph, SymbolNode};
use crate::demo::{DEMO_REPOSITORY, SpawnAgentsRequest, reset_demo, spawn_agents};
use crate::simulation::{SimulationRequest, run_simulation};
use crate::storage::{DEFAULT_DATABASE_URL, Storage};
use crate::lang::RenderedFile;
use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use serde_json::json;
use std::{net::SocketAddr, path::PathBuf, sync::Arc};
use tokio::fs;

/// The reference-edge kind the TypeScript plugin uses for call relationships. Lives in the
/// TS-aware CLI layer rather than `core`, which is language-agnostic.
const CALL_REFERENCE_KIND: &str = "calls";

fn select_callers(graph: &SemanticGraph, symbol_id: uuid::Uuid) -> Vec<&SymbolNode> {
    graph.find_callers(symbol_id, CALL_REFERENCE_KIND)
}

fn select_callees(graph: &SemanticGraph, symbol_id: uuid::Uuid) -> Vec<&SymbolNode> {
    graph.find_callees(symbol_id, CALL_REFERENCE_KIND)
}

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
    #[arg(long)]
    branch: String,
    #[arg(long, default_value = "Apply edited TypeScript slice")]
    title: String,
    #[arg(long, default_value = "agent")]
    agent: String,
    #[arg(long)]
    original: PathBuf,
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
                Storage::connect(&cli.database_url, Arc::new(crate::ts::TypeScriptPlugin)).await?;
            storage.migrate().await?;
            run_storage_command(storage, command).await
        }
    }
}

async fn run_storage_command(storage: Storage, command: Command) -> Result<()> {
    match command {
        Command::Server(_) => unreachable!("handled before storage command dispatch"),
        Command::Init(args) => {
            let (repository, branch) = storage.init_repository(&args.name).await?;
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &json!({ "repository": repository, "branch": branch })
                )?
            );
        }
        Command::Import(args) => {
            let (repository, branch) = if args.reset {
                storage.reset_repository(&args.repo).await?
            } else {
                let (repository, _) = storage.init_repository(&args.repo).await?;
                let branch = storage.branch_by_name(repository.id, &args.branch).await?;
                (repository, branch)
            };
            let files = storage.plugin().read_source_tree(&args.path)?;
            let operations = storage.plugin().import(&files)?;
            let task = storage
                .create_task(
                    repository.id,
                    &format!("Import TypeScript from {}", args.path.display()),
                )
                .await?;
            let changeset = storage
                .create_changeset(
                    repository.id,
                    task.id,
                    branch.id,
                    "Import TypeScript repository",
                    "typescript-importer",
                )
                .await?;
            storage
                .add_attachment(
                    repository.id,
                    "task",
                    task.id,
                    "PromptAttachment",
                    json!({
                        "model": "typescript-importer",
                        "prompt": format!("Import TypeScript source tree from {}", args.path.display())
                    }),
                )
                .await?;
            let mut appended = Vec::new();
            for operation in operations {
                appended.push(
                    storage
                        .append_operation(repository.id, branch.id, changeset.id, operation)
                        .await?,
                );
            }
            let materialized = storage.materialize_branch(&args.repo, &args.branch).await?;
            materialized.graph.validate()?;
            if !args.no_validate {
                storage.plugin().validate(&materialized.files).await?;
            }
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "repository": repository,
                    "branch": branch,
                    "filesImported": files.len(),
                    "operationsAppended": appended.len(),
                    "symbols": materialized.graph.symbols.len(),
                    "references": materialized.graph.references.len(),
                    "validated": !args.no_validate
                }))?
            );
        }
        Command::Branch { command } => match command {
            BranchCommand::Create(args) => {
                let repository = storage.repository_by_name(&args.repo).await?;
                let branch = storage
                    .create_branch(repository.id, &args.name, &args.base)
                    .await?;
                println!("{}", serde_json::to_string_pretty(&branch)?);
            }
        },
        Command::Task { command } => match command {
            TaskCommand::Create(args) => {
                let repository = storage.repository_by_name(&args.repo).await?;
                let task = storage.create_task(repository.id, &args.title).await?;
                println!("{}", serde_json::to_string_pretty(&task)?);
            }
        },
        Command::Slice { command } => match command {
            SliceCommand::Create(args) => {
                let materialized = storage.materialize_branch(&args.repo, &args.branch).await?;
                let root_symbols = if let Some(name) = args.symbol {
                    materialized
                        .graph
                        .find_symbol(&name)
                        .into_iter()
                        .map(|symbol| symbol.id)
                        .collect()
                } else {
                    Vec::new()
                };
                let slice = storage.plugin().render_slice(
                    &materialized.graph,
                    format!(
                        "{}@{}",
                        materialized.branch.name,
                        materialized.operations.len()
                    ),
                    root_symbols,
                );
                println!("{}", serde_json::to_string_pretty(&slice)?);
            }
            SliceCommand::Apply(args) => {
                let repository = storage.repository_by_name(&args.repo).await?;
                let branch = storage.branch_by_name(repository.id, &args.branch).await?;
                let original = read_rendered_files(&args.original).await?;
                let modified = read_rendered_files(&args.modified).await?;
                let operations = storage.plugin().diff(&original, &modified)?;
                let task = storage.create_task(repository.id, &args.title).await?;
                let changeset = storage
                    .create_changeset(repository.id, task.id, branch.id, &args.title, &args.agent)
                    .await?;
                let mut appended = Vec::new();
                for operation in operations {
                    appended.push(
                        storage
                            .append_operation(repository.id, branch.id, changeset.id, operation)
                            .await?,
                    );
                }
                println!("{}", serde_json::to_string_pretty(&appended)?);
            }
        },
        Command::Merge(args) => {
            let result = storage
                .merge_branch(&args.repo, &args.source, &args.target)
                .await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Command::Validate(args) => {
            let materialized = storage.materialize_branch(&args.repo, &args.branch).await?;
            materialized.graph.validate()?;
            storage.plugin().validate(&materialized.files).await?;
            println!(
                "OK {}@{}: {} symbols, {} references",
                materialized.branch.name,
                materialized.operations.len(),
                materialized.graph.symbols.len(),
                materialized.graph.references.len()
            );
        }
        Command::Render(args) => {
            let materialized = storage.materialize_branch(&args.repo, &args.branch).await?;
            for file in materialized.files {
                let output_path = args.out.join(&file.path);
                if let Some(parent) = output_path.parent() {
                    fs::create_dir_all(parent).await?;
                }
                fs::write(&output_path, file.content).await?;
            }
            println!("rendered {} to {}", args.branch, args.out.display());
        }
        Command::Simulate(args) => {
            let result = run_simulation(
                &storage,
                SimulationRequest {
                    agent_count: args.agents,
                    include_conflicts: args.conflicts,
                },
            )
            .await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Command::Query { command } => match command {
            QueryCommand::FindSymbol(args) => {
                let materialized = storage.materialize_branch(&args.repo, &args.branch).await?;
                let symbols = materialized.graph.find_symbol(&args.name);
                println!("{}", serde_json::to_string_pretty(&symbols)?);
            }
            QueryCommand::FindReferences(args) => {
                let materialized = storage.materialize_branch(&args.repo, &args.branch).await?;
                let symbol = resolve_symbol(&materialized.graph, &args.name)?;
                let references = materialized.graph.find_references(symbol.id);
                println!("{}", serde_json::to_string_pretty(&references)?);
            }
            QueryCommand::FindCallers(args) => {
                print_related_symbols(&storage, &args, select_callers).await?
            }
            QueryCommand::FindCallees(args) => {
                print_related_symbols(&storage, &args, select_callees).await?
            }
            QueryCommand::FindDependencies(args) => {
                print_related_symbols(&storage, &args, SemanticGraph::find_dependencies).await?
            }
            QueryCommand::FindDependents(args) => {
                print_related_symbols(&storage, &args, SemanticGraph::find_dependents).await?
            }
        },
        Command::Demo { command } => match command {
            DemoCommand::Reset => {
                let state = reset_demo(&storage).await?;
                println!("{}", serde_json::to_string_pretty(&state.metrics)?);
            }
            DemoCommand::Spawn(args) => {
                let state = spawn_agents(
                    &storage,
                    SpawnAgentsRequest {
                        count: args.count,
                        include_conflicts: args.conflicts,
                    },
                )
                .await?;
                println!("{}", serde_json::to_string_pretty(&state.metrics)?);
            }
            DemoCommand::MergeAll => {
                let mut results = Vec::new();
                loop {
                    let state = crate::demo::demo_state(&storage).await?;
                    let Some(branch) = state
                        .branches
                        .iter()
                        .filter(|branch| branch.status == crate::demo::BranchStatus::Ready)
                        .min_by(|a, b| a.name.cmp(&b.name))
                    else {
                        break;
                    };
                    let result = storage
                        .merge_branch(DEMO_REPOSITORY, &branch.name, "main")
                        .await?;
                    let conflicted = !result.conflicts.is_empty();
                    results.push(result);
                    if conflicted {
                        break;
                    }
                }
                println!("{}", serde_json::to_string_pretty(&results)?);
            }
        },
    }

    Ok(())
}

fn resolve_symbol<'g>(graph: &'g SemanticGraph, name: &str) -> Result<&'g SymbolNode> {
    graph
        .find_symbol(name)
        .first()
        .copied()
        .with_context(|| format!("symbol {name} not found"))
}

/// Shared driver for the relationship queries that resolve a symbol by name and print the symbols
/// it relates to (callers/callees/dependencies/dependents), passed as a graph method.
async fn print_related_symbols(
    storage: &Storage,
    args: &FindSymbolArgs,
    select: fn(&SemanticGraph, uuid::Uuid) -> Vec<&SymbolNode>,
) -> Result<()> {
    let materialized = storage.materialize_branch(&args.repo, &args.branch).await?;
    let symbol = resolve_symbol(&materialized.graph, &args.name)?;
    let related = select(&materialized.graph, symbol.id);
    println!("{}", serde_json::to_string_pretty(&related)?);
    Ok(())
}

async fn read_rendered_files(path: &PathBuf) -> Result<Vec<RenderedFile>> {
    let raw = fs::read_to_string(path).await?;
    if let Ok(files) = serde_json::from_str::<Vec<RenderedFile>>(&raw) {
        return Ok(files);
    }

    if let Ok(file) = serde_json::from_str::<RenderedFile>(&raw) {
        return Ok(vec![file]);
    }

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("slice.ts")
        .to_string();
    Ok(vec![RenderedFile {
        path: file_name,
        content: raw,
    }])
}
