use crate::demo::{DEMO_REPOSITORY, SpawnAgentsRequest, reset_demo, spawn_agents};
use crate::simulation::{SimulationRequest, run_simulation};
use anyhow::{Context, Result, bail};
use bonhomme_core::{
    Branch, MergeOutcome, Operation, OperationRecord, RenderedFile, Repository, SemanticGraph,
    metadata_string, safe_relative_path,
};
use bonhomme_engine::{MaterializedBranch, PendingSourceFileSnapshot, SourceFileSnapshot, Storage};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};
use tokio::fs;
use uuid::Uuid;

use super::files::{read_rendered_files, read_slice_id};
use super::queries::{
    print_related_symbols, resolve_symbol, select_callees, select_callers, select_dependencies,
    select_dependents,
};
use super::slice_audit::{SliceAuditContext, slice_recovery_audit};
use super::{
    BranchCommand, Command, DemoCommand, ImportArgs, QueryCommand, SliceCommand, TaskCommand,
};

const IMPORTER_VERSION: &str = "source-snapshot-v1";

async fn resolve_repository_name(root: &Path, explicit: Option<&str>) -> Result<String> {
    if let Some(repo) = explicit {
        return Ok(repo.to_string());
    }
    if let Some(repo) = super::session::active_repository_name(root).await? {
        return Ok(repo);
    }
    super::default_repository_name(root)
}

/// Count root file symbols by their handler tag for the `import` report ("5 typescript, 3 blob").
/// Files degraded to the blob handler show up as `blob`, so degradation is visible, never silent.
fn handler_breakdown(graph: &SemanticGraph) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for symbol in graph.root_symbols() {
        if symbol.kind == "file" {
            let handler = metadata_string(&symbol.metadata, "handler")
                .unwrap_or_else(|| "untagged".to_string());
            *counts.entry(handler).or_insert(0) += 1;
        }
    }
    counts
}

#[derive(Debug, Default)]
struct ImportStats {
    mode: &'static str,
    files_scanned: usize,
    files_unchanged: usize,
    files_changed: usize,
    files_added: usize,
    files_deleted: usize,
}

struct IncrementalPlan {
    unchanged: Vec<String>,
    changed: Vec<RenderedFile>,
    added: Vec<RenderedFile>,
    deleted: Vec<SourceFileSnapshot>,
}

async fn run_import_command(storage: Storage, args: ImportArgs, root: &Path) -> Result<()> {
    let repository_name = resolve_repository_name(root, args.repo.as_deref()).await?;
    let (repository, branch) = import_target(&storage, &args, &repository_name).await?;
    let files = storage.plugin().read_source_tree(&args.path)?;
    let existing_snapshots = if args.reset {
        Vec::new()
    } else {
        storage.list_source_file_snapshots(branch.id).await?
    };
    let existing_operations = if args.reset {
        Vec::new()
    } else {
        storage.collect_branch_operations(branch.id, None).await?
    };

    if !args.reset && existing_snapshots.is_empty() && !existing_operations.is_empty() {
        bail!(
            "repository {} branch {} has no source file snapshots; run `bonhomme import --repo {} --branch {} --path {} --reset` once to seed incremental import state",
            repository_name,
            args.branch,
            repository_name,
            args.branch,
            args.path.display()
        );
    }

    let (appended, materialized, stats) = if existing_snapshots.is_empty() {
        full_import(
            &storage,
            &repository,
            &branch,
            &args,
            &repository_name,
            &files,
        )
        .await?
    } else {
        incremental_import(
            &storage,
            &repository,
            &branch,
            &args,
            &repository_name,
            &files,
            &existing_snapshots,
        )
        .await?
    };

    materialized.graph.validate()?;
    let (validated, validation_error) = validate_after_import(
        &storage,
        &materialized,
        args.no_validate,
        appended.is_empty(),
    )
    .await;
    let snapshots = snapshots_from_files(
        &files,
        &materialized.graph,
        materialized.operations.len() as i64,
    );
    storage
        .replace_source_file_snapshots(repository.id, branch.id, snapshots)
        .await?;

    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "repository": repository,
            "branch": branch,
            "mode": stats.mode,
            "filesImported": files.len(),
            "filesScanned": stats.files_scanned,
            "filesUnchanged": stats.files_unchanged,
            "filesChanged": stats.files_changed,
            "filesAdded": stats.files_added,
            "filesDeleted": stats.files_deleted,
            "operationsAppended": appended.len(),
            "symbols": materialized.graph.symbols.len(),
            "references": materialized.graph.references.len(),
            "handlerBreakdown": handler_breakdown(&materialized.graph),
            "validated": validated,
            "validationError": validation_error
        }))?
    );

    Ok(())
}

async fn import_target(
    storage: &Storage,
    args: &ImportArgs,
    repository_name: &str,
) -> Result<(Repository, Branch)> {
    let (repository, main) = if args.reset {
        storage.reset_repository(repository_name).await?
    } else {
        storage.init_repository(repository_name).await?
    };
    if args.branch == main.name {
        return Ok((repository, main));
    }
    let branch = if args.reset {
        storage
            .create_branch(repository.id, &args.branch, &main.name)
            .await?
    } else {
        storage.branch_by_name(repository.id, &args.branch).await?
    };
    Ok((repository, branch))
}

async fn full_import(
    storage: &Storage,
    repository: &Repository,
    branch: &Branch,
    args: &ImportArgs,
    repository_name: &str,
    files: &[RenderedFile],
) -> Result<(Vec<OperationRecord>, MaterializedBranch, ImportStats)> {
    let operations = storage.plugin().import(files)?;
    let appended = append_import_operations(
        storage,
        repository,
        branch,
        args,
        "Import repository",
        operations,
    )
    .await?;
    let materialized = storage
        .materialize_branch(repository_name, &args.branch)
        .await?;
    let stats = ImportStats {
        mode: "full",
        files_scanned: files.len(),
        files_changed: files.len(),
        files_added: files.len(),
        ..ImportStats::default()
    };
    Ok((appended, materialized, stats))
}

async fn incremental_import(
    storage: &Storage,
    repository: &Repository,
    branch: &Branch,
    args: &ImportArgs,
    repository_name: &str,
    files: &[RenderedFile],
    existing_snapshots: &[SourceFileSnapshot],
) -> Result<(Vec<OperationRecord>, MaterializedBranch, ImportStats)> {
    let plan = plan_incremental_import(files, existing_snapshots);
    let base = storage
        .materialize_branch(repository_name, &args.branch)
        .await?;
    let file_symbols = file_symbols_by_path(&base.graph);
    let changed_scope = plan
        .changed
        .iter()
        .filter_map(|file| file_symbols.get(&file.path).map(|file| file.id))
        .collect::<Vec<_>>();

    let mut operations = Vec::new();
    let delete_operations = delete_file_operations(
        &base.graph,
        plan.deleted.iter().map(|snapshot| snapshot.path.as_str()),
    );
    let edited = plan
        .changed
        .iter()
        .chain(plan.added.iter())
        .cloned()
        .collect::<Vec<_>>();
    let recovered = if changed_scope.is_empty() {
        storage.plugin().import(&plan.added)?
    } else {
        storage
            .plugin()
            .recover_operations(&base.graph, &changed_scope, &edited)?
    };
    operations.extend(merge_delete_and_recovered_operations(
        delete_operations,
        recovered,
    ));

    let appended = append_import_operations(
        storage,
        repository,
        branch,
        args,
        "Incremental import repository",
        operations,
    )
    .await?;
    let materialized = if appended.is_empty() {
        base
    } else {
        storage
            .materialize_branch(repository_name, &args.branch)
            .await?
    };
    let stats = ImportStats {
        mode: "incremental",
        files_scanned: files.len(),
        files_unchanged: plan.unchanged.len(),
        files_changed: plan.changed.len(),
        files_added: plan.added.len(),
        files_deleted: plan.deleted.len(),
    };
    Ok((appended, materialized, stats))
}

async fn append_import_operations(
    storage: &Storage,
    repository: &Repository,
    branch: &Branch,
    args: &ImportArgs,
    title: &str,
    operations: Vec<Operation>,
) -> Result<Vec<OperationRecord>> {
    if operations.is_empty() {
        return Ok(Vec::new());
    }
    let task = storage
        .create_task(
            repository.id,
            &format!("{title} from {}", args.path.display()),
        )
        .await?;
    let changeset = storage
        .create_changeset(
            repository.id,
            task.id,
            branch.id,
            title,
            "bonhomme-importer",
        )
        .await?;
    storage
        .add_attachment(
            repository.id,
            "task",
            task.id,
            "PromptAttachment",
            json!({
                "model": "bonhomme-importer",
                "prompt": format!("{title} from {}", args.path.display())
            }),
        )
        .await?;
    storage
        .append_operations(repository.id, branch.id, changeset.id, operations)
        .await
}

async fn validate_after_import(
    storage: &Storage,
    materialized: &MaterializedBranch,
    no_validate: bool,
    no_changes: bool,
) -> (bool, Option<String>) {
    if no_validate || no_changes {
        return (false, None);
    }
    match storage.plugin().validate(&materialized.files).await {
        Ok(()) => (true, None),
        Err(error) => {
            let message = compact_error(&error);
            eprintln!("bonhomme: validation failed after import: {message}");
            (false, Some(message))
        }
    }
}

fn plan_incremental_import(
    files: &[RenderedFile],
    existing_snapshots: &[SourceFileSnapshot],
) -> IncrementalPlan {
    let existing = existing_snapshots
        .iter()
        .map(|snapshot| (snapshot.path.as_str(), snapshot))
        .collect::<BTreeMap<_, _>>();
    let current = files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect::<BTreeMap<_, _>>();
    let mut plan = IncrementalPlan {
        unchanged: Vec::new(),
        changed: Vec::new(),
        added: Vec::new(),
        deleted: Vec::new(),
    };

    for file in files {
        match existing.get(file.path.as_str()) {
            Some(snapshot)
                if snapshot.importer_version == IMPORTER_VERSION
                    && snapshot.content_hash == content_hash(&file.content) =>
            {
                plan.unchanged.push(file.path.clone());
            }
            Some(_) => plan.changed.push(file.clone()),
            None => plan.added.push(file.clone()),
        }
    }
    for snapshot in existing_snapshots {
        if !current.contains_key(snapshot.path.as_str()) {
            plan.deleted.push(snapshot.clone());
        }
    }

    plan
}

#[derive(Clone)]
struct FileSymbolInfo {
    id: Uuid,
    handler: String,
}

fn file_symbols_by_path(graph: &SemanticGraph) -> BTreeMap<String, FileSymbolInfo> {
    graph
        .root_symbols()
        .into_iter()
        .filter(|symbol| symbol.kind == "file")
        .map(|symbol| {
            let path =
                metadata_string(&symbol.metadata, "path").unwrap_or_else(|| symbol.name.clone());
            let handler = metadata_string(&symbol.metadata, "handler")
                .unwrap_or_else(|| "untagged".to_string());
            (
                path,
                FileSymbolInfo {
                    id: symbol.id,
                    handler,
                },
            )
        })
        .collect()
}

fn snapshots_from_files(
    files: &[RenderedFile],
    graph: &SemanticGraph,
    last_import_position: i64,
) -> Vec<PendingSourceFileSnapshot> {
    let file_symbols = file_symbols_by_path(graph);
    files
        .iter()
        .map(|file| {
            let symbol = file_symbols.get(&file.path);
            PendingSourceFileSnapshot {
                path: file.path.clone(),
                content_hash: content_hash(&file.content),
                byte_len: file.content.len() as i64,
                handler: symbol
                    .map(|info| info.handler.clone())
                    .unwrap_or_else(|| "unknown".to_string()),
                file_symbol_id: symbol.map(|info| info.id),
                last_import_position,
                importer_version: IMPORTER_VERSION.to_string(),
            }
        })
        .collect()
}

fn content_hash(content: &str) -> String {
    let digest = Sha256::digest(content.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn delete_file_operations<'a>(
    graph: &SemanticGraph,
    paths: impl IntoIterator<Item = &'a str>,
) -> Vec<Operation> {
    let file_symbols = file_symbols_by_path(graph);
    let mut delete_ids = BTreeSet::new();
    let mut symbol_deletes = Vec::new();
    for path in paths {
        if let Some(file) = file_symbols.get(path) {
            collect_delete_symbols(graph, file.id, &mut delete_ids, &mut symbol_deletes);
        }
    }
    let mut reference_deletes = graph
        .references
        .values()
        .filter(|reference| {
            delete_ids.contains(&reference.from_symbol_id)
                || delete_ids.contains(&reference.to_symbol_id)
        })
        .map(|reference| (reference.ordinal, reference.id))
        .collect::<Vec<_>>();
    reference_deletes.sort();

    reference_deletes
        .into_iter()
        .map(|(_, reference_id)| Operation::DeleteReference { reference_id })
        .chain(
            symbol_deletes
                .into_iter()
                .map(|symbol_id| Operation::DeleteSymbol { symbol_id }),
        )
        .collect()
}

fn collect_delete_symbols(
    graph: &SemanticGraph,
    symbol_id: Uuid,
    delete_ids: &mut BTreeSet<Uuid>,
    symbol_deletes: &mut Vec<Uuid>,
) {
    if !delete_ids.insert(symbol_id) {
        return;
    }
    for child in graph.children_of(symbol_id) {
        collect_delete_symbols(graph, child.id, delete_ids, symbol_deletes);
    }
    symbol_deletes.push(symbol_id);
}

fn merge_delete_and_recovered_operations(
    delete_operations: Vec<Operation>,
    recovered_operations: Vec<Operation>,
) -> Vec<Operation> {
    let mut seen_reference_deletes = BTreeSet::new();
    let mut seen_symbol_deletes = BTreeSet::new();
    let mut reference_deletes = Vec::new();
    let mut symbol_deletes = Vec::new();
    let mut rest = Vec::new();

    for operation in delete_operations.into_iter().chain(recovered_operations) {
        match operation {
            Operation::DeleteReference { reference_id } => {
                if seen_reference_deletes.insert(reference_id) {
                    reference_deletes.push(Operation::DeleteReference { reference_id });
                }
            }
            Operation::DeleteSymbol { symbol_id } => {
                if seen_symbol_deletes.insert(symbol_id) {
                    symbol_deletes.push(Operation::DeleteSymbol { symbol_id });
                }
            }
            other => rest.push(other),
        }
    }

    reference_deletes
        .into_iter()
        .chain(symbol_deletes)
        .chain(rest)
        .collect()
}

pub(super) async fn run_storage_command(
    storage: Storage,
    command: Command,
    root: &Path,
) -> Result<()> {
    match command {
        Command::Agents(_) => unreachable!("handled before storage command dispatch"),
        Command::Server(_) => unreachable!("handled before storage command dispatch"),
        Command::Explore(_) => unreachable!("handled before storage command dispatch"),
        Command::Session { .. } => unreachable!("handled before storage command dispatch"),
        Command::Init(args) => {
            let name = resolve_repository_name(root, args.name.as_deref()).await?;
            let (repository, branch) = storage.init_repository(&name).await?;
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &json!({ "repository": repository, "branch": branch })
                )?
            );
        }
        Command::Import(args) => run_import_command(storage, args, root).await?,
        Command::Branch { command } => match command {
            BranchCommand::Create(args) => {
                let repo = resolve_repository_name(root, args.repo.as_deref()).await?;
                let repository = storage.repository_by_name(&repo).await?;
                let branch = storage
                    .create_branch(repository.id, &args.name, &args.base)
                    .await?;
                println!("{}", serde_json::to_string_pretty(&branch)?);
            }
        },
        Command::Task { command } => match command {
            TaskCommand::Create(args) => {
                let repo = resolve_repository_name(root, args.repo.as_deref()).await?;
                let repository = storage.repository_by_name(&repo).await?;
                let task = storage.create_task(repository.id, &args.title).await?;
                println!("{}", serde_json::to_string_pretty(&task)?);
            }
        },
        Command::Slice { command } => match command {
            SliceCommand::Create(args) => {
                let repo = resolve_repository_name(root, args.repo.as_deref()).await?;
                let materialized = storage.materialize_branch(&repo, &args.branch).await?;
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
                let base_position = materialized.operations.len() as i64;
                let stored_slice = storage
                    .create_slice(
                        materialized.repository.id,
                        materialized.branch.id,
                        base_position,
                        &root_symbols,
                    )
                    .await?;
                let mut slice = storage.plugin().render_slice(
                    &materialized.graph,
                    format!("{}@{}", materialized.branch.name, base_position),
                    root_symbols,
                );
                slice.id = stored_slice.id;
                println!("{}", serde_json::to_string_pretty(&slice)?);
            }
            SliceCommand::Apply(args) => {
                let repo = resolve_repository_name(root, args.repo.as_deref()).await?;
                let repository = storage.repository_by_name(&repo).await?;
                let modified_slice_id = read_slice_id(&args.modified).await?;
                let modified = read_rendered_files(&args.modified).await?;
                let slice_id = args.slice_id.or(modified_slice_id);
                let (branch, operations, audit_context) = if let Some(slice_id) = slice_id {
                    let stored_slice = storage.slice_by_id(slice_id).await?;
                    if stored_slice.repository_id != repository.id {
                        bail!("slice {slice_id} does not belong to repository {}", repo);
                    }
                    let branch = storage.branch_by_id(stored_slice.branch_id).await?;
                    let branch_position_at_apply = storage
                        .collect_branch_operations(branch.id, None)
                        .await?
                        .len() as i64;
                    let materialized = storage
                        .materialize_branch_at_position(branch.id, stored_slice.base_position)
                        .await?;
                    let operations = storage.plugin().recover_operations(
                        &materialized.graph,
                        &stored_slice.root_symbols,
                        &modified,
                    )?;
                    // Collapse delete+create pairs that are actually a move into identity-preserving
                    // MoveSymbol ops, so a symbol relocated between files/classes keeps its id. Run
                    // before the merge analysis so conflict detection sees the real move operations.
                    let operations = bonhomme_core::detect_moves(operations, &materialized.graph);
                    let analysis = storage
                        .analyze_operations_against_branch(
                            branch.id,
                            stored_slice.base_position,
                            &operations,
                        )
                        .await?;
                    if analysis.outcome == MergeOutcome::Conflict {
                        bail!(
                            "slice {slice_id} conflicts with branch {}: {}",
                            branch.name,
                            serde_json::to_string_pretty(&analysis.conflicts)?
                        );
                    }
                    let audit_context = SliceAuditContext {
                        slice_id,
                        base_position: stored_slice.base_position,
                        branch_position_at_apply,
                        root_symbols: stored_slice.root_symbols,
                    };
                    (branch, operations, Some(audit_context))
                } else {
                    let branch = storage.branch_by_name(repository.id, &args.branch).await?;
                    let original_path = args
                        .original
                        .as_ref()
                        .context("slice apply without --slice-id requires --original")?;
                    let original = read_rendered_files(original_path).await?;
                    let operations = storage.plugin().diff(&original, &modified)?;
                    // Build a base graph from the original so a relocated symbol is detected as a
                    // move on the legacy two-blob diff path too. Best-effort: if the original will
                    // not import cleanly, fall back to the raw diff.
                    let operations = match storage.plugin().import(&original) {
                        Ok(base_ops) => {
                            let mut base_graph = SemanticGraph::default();
                            let built = base_ops
                                .iter()
                                .try_for_each(|op| base_graph.apply_operation(Uuid::new_v4(), op));
                            match built {
                                Ok(()) => bonhomme_core::detect_moves(operations, &base_graph),
                                Err(_) => operations,
                            }
                        }
                        Err(_) => operations,
                    };
                    (branch, operations, None)
                };
                let task = storage.create_task(repository.id, &args.title).await?;
                let changeset = storage
                    .create_changeset(repository.id, task.id, branch.id, &args.title, &args.agent)
                    .await?;
                let appended = storage
                    .append_operations(repository.id, branch.id, changeset.id, operations.clone())
                    .await?;
                if let Some(audit_context) = audit_context {
                    storage
                        .add_attachment(
                            repository.id,
                            "changeset",
                            changeset.id,
                            "SliceRecoveryAttachment",
                            slice_recovery_audit(
                                &audit_context,
                                &branch.name,
                                &operations,
                                &appended,
                            ),
                        )
                        .await?;
                }
                println!("{}", serde_json::to_string_pretty(&appended)?);
            }
        },
        Command::Merge(args) => {
            let repo = resolve_repository_name(root, args.repo.as_deref()).await?;
            let result = storage
                .merge_branch(&repo, &args.source, &args.target)
                .await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Command::Validate(args) => {
            let repo = resolve_repository_name(root, args.repo.as_deref()).await?;
            let materialized = storage.materialize_branch(&repo, &args.branch).await?;
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
            let repo = resolve_repository_name(root, args.repo.as_deref()).await?;
            let materialized = storage.materialize_branch(&repo, &args.branch).await?;
            for file in materialized.files {
                let output_path = args.out.join(safe_relative_path(&file.path)?);
                if let Some(parent) = output_path.parent() {
                    fs::create_dir_all(parent).await?;
                }
                // Binary files round-trip as a base64 envelope in the body; decode back to raw
                // bytes on disk so the rendered tree is faithful, not the envelope text.
                match bonhomme_core::decode_binary(&file.content) {
                    Some(bytes) => fs::write(&output_path, bytes).await?,
                    None => fs::write(&output_path, file.content).await?,
                }
            }
            println!("rendered {} to {}", args.branch, args.out.display());
        }
        Command::Simulate(args) => {
            let result = run_simulation(
                &storage,
                SimulationRequest {
                    agent_count: args.agents,
                    include_conflicts: args.conflicts,
                    language: args.language,
                },
            )
            .await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Command::Query { command } => match command {
            QueryCommand::FindSymbol(args) => {
                let repo = resolve_repository_name(root, args.repo.as_deref()).await?;
                let materialized = storage.materialize_branch(&repo, &args.branch).await?;
                let symbols = materialized.graph.find_symbol(&args.name);
                println!("{}", serde_json::to_string_pretty(&symbols)?);
            }
            QueryCommand::FindReferences(args) => {
                let repo = resolve_repository_name(root, args.repo.as_deref()).await?;
                let materialized = storage.materialize_branch(&repo, &args.branch).await?;
                let symbol = resolve_symbol(&materialized.graph, &args.name)?;
                let references = materialized.graph.find_references(symbol.id);
                println!("{}", serde_json::to_string_pretty(&references)?);
            }
            QueryCommand::FindCallers(args) => {
                let repo = resolve_repository_name(root, args.repo.as_deref()).await?;
                print_related_symbols(&storage, &repo, &args, select_callers).await?
            }
            QueryCommand::FindCallees(args) => {
                let repo = resolve_repository_name(root, args.repo.as_deref()).await?;
                print_related_symbols(&storage, &repo, &args, select_callees).await?
            }
            QueryCommand::FindDependencies(args) => {
                let repo = resolve_repository_name(root, args.repo.as_deref()).await?;
                print_related_symbols(&storage, &repo, &args, select_dependencies).await?
            }
            QueryCommand::FindDependents(args) => {
                let repo = resolve_repository_name(root, args.repo.as_deref()).await?;
                print_related_symbols(&storage, &repo, &args, select_dependents).await?
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

fn compact_error(error: &anyhow::Error) -> String {
    const MAX_CHARS: usize = 4000;
    const MAX_LINES: usize = 20;

    let message = format!("{error:#}");
    let line_count = message.lines().count();
    let mut compact = message
        .lines()
        .take(MAX_LINES)
        .collect::<Vec<_>>()
        .join("\n");
    if line_count > MAX_LINES {
        compact.push_str("\n...");
    }
    if compact.len() > MAX_CHARS {
        compact.truncate(MAX_CHARS);
        compact.push_str("...");
    }
    compact
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use serde_json::json;

    fn rendered(path: &str, content: &str) -> RenderedFile {
        RenderedFile {
            path: path.to_string(),
            content: content.to_string(),
        }
    }

    fn snapshot(path: &str, content: &str, importer_version: &str) -> SourceFileSnapshot {
        SourceFileSnapshot {
            repository_id: Uuid::new_v4(),
            branch_id: Uuid::new_v4(),
            path: path.to_string(),
            content_hash: content_hash(content),
            byte_len: content.len() as i64,
            handler: "blob".to_string(),
            file_symbol_id: Some(Uuid::new_v4()),
            last_import_position: 1,
            importer_version: importer_version.to_string(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn incremental_plan_classifies_unchanged_changed_added_and_deleted_files() {
        let files = vec![
            rendered("keep.txt", "same"),
            rendered("change.txt", "new"),
            rendered("fresh.txt", "fresh"),
        ];
        let snapshots = vec![
            snapshot("keep.txt", "same", IMPORTER_VERSION),
            snapshot("change.txt", "old", IMPORTER_VERSION),
            snapshot("gone.txt", "gone", IMPORTER_VERSION),
            snapshot("version.txt", "same", "older"),
        ];

        let plan = plan_incremental_import(&files, &snapshots);

        assert_eq!(plan.unchanged, vec!["keep.txt"]);
        assert_eq!(
            plan.changed
                .iter()
                .map(|file| file.path.as_str())
                .collect::<Vec<_>>(),
            vec!["change.txt"]
        );
        assert_eq!(
            plan.added
                .iter()
                .map(|file| file.path.as_str())
                .collect::<Vec<_>>(),
            vec!["fresh.txt"]
        );
        assert_eq!(
            plan.deleted
                .iter()
                .map(|snapshot| snapshot.path.as_str())
                .collect::<Vec<_>>(),
            vec!["gone.txt", "version.txt"]
        );
    }

    #[test]
    fn delete_file_operations_remove_references_before_child_symbols() {
        let file_id = Uuid::new_v4();
        let child_id = Uuid::new_v4();
        let other_id = Uuid::new_v4();
        let reference_id = Uuid::new_v4();
        let mut graph = SemanticGraph::default();
        graph
            .apply_operation(
                Uuid::new_v4(),
                &Operation::CreateSymbol {
                    symbol_id: file_id,
                    parent_id: None,
                    kind: "file".to_string(),
                    name: "src/lib.ts".to_string(),
                    body: None,
                    metadata: json!({"handler": "typescript", "path": "src/lib.ts"}),
                },
            )
            .unwrap();
        graph
            .apply_operation(
                Uuid::new_v4(),
                &Operation::CreateSymbol {
                    symbol_id: child_id,
                    parent_id: Some(file_id),
                    kind: "function".to_string(),
                    name: "lib".to_string(),
                    body: Some("return 1;".to_string()),
                    metadata: json!({}),
                },
            )
            .unwrap();
        graph
            .apply_operation(
                Uuid::new_v4(),
                &Operation::CreateSymbol {
                    symbol_id: other_id,
                    parent_id: None,
                    kind: "file".to_string(),
                    name: "src/main.ts".to_string(),
                    body: None,
                    metadata: json!({"handler": "typescript", "path": "src/main.ts"}),
                },
            )
            .unwrap();
        graph
            .apply_operation(
                Uuid::new_v4(),
                &Operation::CreateReference {
                    reference_id,
                    from_symbol_id: other_id,
                    to_symbol_id: child_id,
                    kind: "calls".to_string(),
                },
            )
            .unwrap();

        let operations = delete_file_operations(&graph, ["src/lib.ts"]);

        assert!(matches!(
            operations.as_slice(),
            [
                Operation::DeleteReference { reference_id: first },
                Operation::DeleteSymbol { symbol_id: child },
                Operation::DeleteSymbol { symbol_id: file },
            ] if *first == reference_id && *child == child_id && *file == file_id
        ));
    }
}
