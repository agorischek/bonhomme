use crate::demo::{DEMO_REPOSITORY, SpawnAgentsRequest, reset_demo, spawn_agents};
use crate::simulation::{SimulationRequest, run_simulation};
use anyhow::{Context, Result, bail};
use bonhomme_core::{MergeOutcome, SemanticGraph, metadata_string};
use bonhomme_engine::Storage;
use serde_json::json;
use std::collections::BTreeMap;
use tokio::fs;

use super::files::read_rendered_files;
use super::queries::{
    print_related_symbols, resolve_symbol, select_callees, select_callers, select_dependencies,
    select_dependents,
};
use super::slice_audit::{SliceAuditContext, slice_recovery_audit};
use super::{BranchCommand, Command, DemoCommand, QueryCommand, SliceCommand, TaskCommand};

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

pub(super) async fn run_storage_command(storage: Storage, command: Command) -> Result<()> {
    match command {
        Command::Server(_) => unreachable!("handled before storage command dispatch"),
        Command::Explore(_) => unreachable!("handled before storage command dispatch"),
        Command::Session { .. } => unreachable!("handled before storage command dispatch"),
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
                    &format!("Import source tree from {}", args.path.display()),
                )
                .await?;
            let changeset = storage
                .create_changeset(
                    repository.id,
                    task.id,
                    branch.id,
                    "Import repository",
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
                        "prompt": format!("Import source tree from {}", args.path.display())
                    }),
                )
                .await?;
            let appended = storage
                .append_operations(repository.id, branch.id, changeset.id, operations)
                .await?;
            let materialized = storage.materialize_branch(&args.repo, &args.branch).await?;
            materialized.graph.validate()?;
            let mut validated = false;
            let mut validation_error = None;
            if !args.no_validate {
                match storage.plugin().validate(&materialized.files).await {
                    Ok(()) => validated = true,
                    Err(error) => {
                        let message = compact_error(&error);
                        eprintln!("bonhomme: validation failed after import: {message}");
                        validation_error = Some(message);
                    }
                }
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
                    "handlerBreakdown": handler_breakdown(&materialized.graph),
                    "validated": validated,
                    "validationError": validation_error
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
                let repository = storage.repository_by_name(&args.repo).await?;
                let modified = read_rendered_files(&args.modified).await?;
                let (branch, operations, audit_context) = if let Some(slice_id) = args.slice_id {
                    let stored_slice = storage.slice_by_id(slice_id).await?;
                    if stored_slice.repository_id != repository.id {
                        bail!(
                            "slice {slice_id} does not belong to repository {}",
                            args.repo
                        );
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
                    (branch, operations, None)
                };
                let task = storage.create_task(repository.id, &args.title).await?;
                let changeset = storage
                    .create_changeset(repository.id, task.id, branch.id, &args.title, &args.agent)
                    .await?;
                let mut appended = Vec::new();
                for operation in &operations {
                    appended.push(
                        storage
                            .append_operation(
                                repository.id,
                                branch.id,
                                changeset.id,
                                operation.clone(),
                            )
                            .await?,
                    );
                }
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
                print_related_symbols(&storage, &args, select_dependencies).await?
            }
            QueryCommand::FindDependents(args) => {
                print_related_symbols(&storage, &args, select_dependents).await?
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
