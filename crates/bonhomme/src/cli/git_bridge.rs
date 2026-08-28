//! Git compatibility and evaluation boundary.
//!
//! Git remains the durable byte-history ledger. This module reads immutable Git objects into
//! disposable directories, evaluates the same three revisions with Git and Bonhomme, and exports
//! Bonhomme state as an ordinary Git tree. None of the shadow paths update refs or the user's
//! index/worktree.

use anyhow::{Context, Result, bail};
use bonhomme_core::{
    LanguagePlugin, MergeOutcome, Operation, OperationBatch, OperationRecord, ProtocolProvenance,
    RenderedFile, analyze_merge, canonical_json_bytes, decode_binary, detect_moves,
    metadata_string, safe_relative_path,
};
use bonhomme_engine::Storage;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsStr,
    fs,
    path::{Component, Path, PathBuf},
    process::{Command, Output},
    sync::OnceLock,
    time::Instant,
};
use uuid::Uuid;

use super::commands::run_import_command;
use super::{GitBenchmarkArgs, GitExportArgs, GitImportArgs, GitShadowArgs, ImportArgs};

const GIT_AUTHOR_NAME: &str = "Bonhomme Benchmark";
const GIT_AUTHOR_EMAIL: &str = "benchmark@invalid.example";
static ISOLATED_GIT_HOME: OnceLock<PathBuf> = OnceLock::new();

#[derive(Debug)]
struct TemporaryDirectory {
    path: PathBuf,
}

impl TemporaryDirectory {
    fn new(label: &str) -> Result<Self> {
        let parent = std::env::temp_dir();
        for _ in 0..32 {
            let path = parent.join(format!("bonhomme-{label}-{}", Uuid::new_v4()));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("could not create {}", path.display()));
                }
            }
        }
        bail!("could not allocate a unique temporary directory")
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum EvaluationOutcome {
    Safe,
    Conflict,
    Error,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GitEvaluation {
    outcome: EvaluationOutcome,
    tree_oid: Option<String>,
    diagnostic: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BonhommeEvaluation {
    outcome: EvaluationOutcome,
    conflicts: usize,
    conflict_reasons: Vec<String>,
    tree_digest: Option<String>,
    base_round_trip_equal: bool,
    base_round_trip_differences: Vec<String>,
    handler_breakdown: BTreeMap<String, usize>,
    blob_paths: Vec<String>,
    validation: ValidationResult,
    diagnostic: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum ValidationResult {
    NotRequested,
    Passed,
    Failed,
    NotApplicable,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ShadowEvaluation {
    base_commit: String,
    ours_commit: String,
    theirs_commit: String,
    git: GitEvaluation,
    bonhomme: BonhommeEvaluation,
    outcomes_agree: bool,
    safe_trees_equal: Option<bool>,
    differing_paths: Vec<String>,
    semantic_candidate: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BenchmarkManifest {
    name: String,
    description: String,
    expected_git: String,
    expected_bonhomme: String,
    #[serde(default = "default_base_directory")]
    base: String,
    #[serde(default = "default_ours_directory")]
    ours: String,
    #[serde(default = "default_theirs_directory")]
    theirs: String,
    expected_tree: Option<String>,
    expected_base_round_trip: Option<bool>,
    #[serde(default)]
    expected_blob_paths: Vec<String>,
    expected_validation: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BenchmarkCaseResult {
    id: String,
    name: String,
    description: String,
    passed: bool,
    failures: Vec<String>,
    unsafe_result: bool,
    elapsed_micros: Option<u128>,
    evaluation: ShadowEvaluation,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BenchmarkReport {
    schema_version: u32,
    corpus: String,
    total: usize,
    passed: usize,
    failed: usize,
    git_clean_merges: usize,
    git_clean_merge_rate: f64,
    bonhomme_clean_merges: usize,
    bonhomme_clean_merge_rate: f64,
    git_conflicts: usize,
    bonhomme_conflicts: usize,
    bonhomme_conflict_records: usize,
    semantic_candidates: usize,
    unsafe_results: usize,
    incorrect_results: usize,
    timing_mode: &'static str,
    elapsed_micros: Option<u128>,
    cases: Vec<BenchmarkCaseResult>,
}

fn default_base_directory() -> String {
    "base".to_string()
}

fn default_ours_directory() -> String {
    "ours".to_string()
}

fn default_theirs_directory() -> String {
    "theirs".to_string()
}

pub(super) async fn run_import(storage: Storage, args: GitImportArgs, root: &Path) -> Result<()> {
    let commit = resolve_commit(&args.git_dir, &args.revision)?;
    let repository_name =
        super::commands::resolve_repository_name(root, args.repo.as_deref()).await?;
    let branch_name = args.branch.clone();
    let requested_revision = args.revision.clone();
    let snapshot = TemporaryDirectory::new("git-import")?;
    materialize_git_tree(&args.git_dir, &format!("{commit}^{{tree}}"), &snapshot.path)?;
    eprintln!(
        "bonhomme: importing immutable Git commit {commit} (requested as {})",
        args.revision
    );
    run_import_command(
        storage.clone(),
        ImportArgs {
            repo: args.repo,
            branch: args.branch,
            path: snapshot.path.clone(),
            reset: args.reset,
            no_validate: args.no_validate,
        },
        root,
    )
    .await?;
    let repository = storage.repository_by_name(&repository_name).await?;
    let branch = storage.branch_by_name(repository.id, &branch_name).await?;
    storage
        .add_attachment(
            repository.id,
            "branch",
            branch.id,
            "GitImportAttachment",
            serde_json::json!({
                "schemaVersion": 1,
                "requestedRevision": requested_revision,
                "commitOid": commit,
            }),
        )
        .await?;
    Ok(())
}

pub(super) async fn run_export(storage: &Storage, args: GitExportArgs, root: &Path) -> Result<()> {
    let repository_name = match args.repo {
        Some(name) => name,
        None => super::commands::resolve_repository_name(root, None).await?,
    };
    prepare_empty_destination(&args.out)?;
    let materialized = storage
        .materialize_branch(&repository_name, &args.branch)
        .await?;
    let operation_batch = export_operation_batch(
        materialized.repository.id,
        materialized.branch.id,
        &materialized.branch.name,
        &materialized.operations,
    )?;
    let landed_files = landed_projection(materialized.files);
    write_rendered_files(&args.out, &landed_files)?;
    init_git_repository(&args.out)?;
    // A rendered `.gitignore` is part of the snapshot, not policy for the export operation.
    run_git_checked(&args.out, ["add", "--force", "--all", "--", "."])?;
    let tree_oid = git_stdout(&args.out, ["write-tree"])?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "schemaVersion": 1,
            "repository": repository_name,
            "branch": args.branch,
            "position": materialized.operations.len(),
            "files": landed_files.len(),
            "out": args.out,
            "treeOid": tree_oid,
            "operationBatch": operation_batch,
        }))?
    );
    Ok(())
}

/// Build the versioned semantic manifest emitted beside every Git tree export. The Git tree remains
/// the byte-compatible artifact; this batch is the replayable semantic provenance boundary.
fn export_operation_batch(
    repository_id: Uuid,
    branch_id: Uuid,
    branch_name: &str,
    records: &[OperationRecord],
) -> Result<OperationBatch> {
    let operations = records
        .iter()
        .map(|record| record.operation.clone())
        .collect::<Vec<_>>();
    let mut identity = b"bonhomme-git-export-operation-batch-v1\0".to_vec();
    identity.extend(repository_id.as_bytes());
    identity.extend(branch_id.as_bytes());
    identity.extend(canonical_json_bytes(&operations)?);
    let batch_id = Uuid::new_v5(&Uuid::NAMESPACE_OID, &identity);
    let created_at = records
        .last()
        .map(|record| record.created_at)
        .unwrap_or(DateTime::<Utc>::UNIX_EPOCH);
    let batch = OperationBatch::new(
        batch_id,
        repository_id,
        branch_id,
        0,
        operations,
        ProtocolProvenance {
            actor: "system:git-export".to_string(),
            tool: format!("bonhomme/{}", env!("CARGO_PKG_VERSION")),
            created_at,
            source_revision: Some(format!("bonhomme:{branch_name}@{}", records.len())),
            extensions: BTreeMap::new(),
        },
    );
    batch.validate_context(repository_id, branch_id, 0)?;
    Ok(batch)
}

pub(super) async fn run_shadow(plugin: &dyn LanguagePlugin, args: GitShadowArgs) -> Result<()> {
    let result = evaluate_revisions(
        plugin,
        &args.git_dir,
        &args.base,
        &args.ours,
        &args.theirs,
        args.validate,
    )
    .await?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

pub(super) async fn run_benchmark(
    plugin: &dyn LanguagePlugin,
    args: GitBenchmarkArgs,
    root: &Path,
) -> Result<()> {
    let corpus = if args.corpus.is_absolute() {
        args.corpus
    } else {
        root.join(args.corpus)
    };
    let report = evaluate_corpus(plugin, &corpus, args.validate, args.measure_time).await?;
    let failed = report.failed;
    println!("{}", serde_json::to_string_pretty(&report)?);
    if failed > 0 {
        bail!("{failed} benchmark case(s) did not match their declared expectations");
    }
    Ok(())
}

async fn evaluate_revisions(
    plugin: &dyn LanguagePlugin,
    git_dir: &Path,
    base: &str,
    ours: &str,
    theirs: &str,
    validate: bool,
) -> Result<ShadowEvaluation> {
    let base_commit = resolve_commit(git_dir, base)?;
    let ours_commit = resolve_commit(git_dir, ours)?;
    let theirs_commit = resolve_commit(git_dir, theirs)?;

    let base_snapshot = TemporaryDirectory::new("shadow-base")?;
    let ours_snapshot = TemporaryDirectory::new("shadow-ours")?;
    let theirs_snapshot = TemporaryDirectory::new("shadow-theirs")?;
    materialize_git_tree(
        git_dir,
        &format!("{base_commit}^{{tree}}"),
        &base_snapshot.path,
    )?;
    materialize_git_tree(
        git_dir,
        &format!("{ours_commit}^{{tree}}"),
        &ours_snapshot.path,
    )?;
    materialize_git_tree(
        git_dir,
        &format!("{theirs_commit}^{{tree}}"),
        &theirs_snapshot.path,
    )?;

    let base_files = plugin.read_source_tree(&base_snapshot.path)?;
    let ours_files = plugin.read_source_tree(&ours_snapshot.path)?;
    let theirs_files = plugin.read_source_tree(&theirs_snapshot.path)?;
    let git = evaluate_git(git_dir, &base_commit, &ours_commit, &theirs_commit)?;
    let (bonhomme, semantic_files) =
        evaluate_bonhomme(plugin, &base_files, &ours_files, &theirs_files, validate).await;

    let mut safe_trees_equal = None;
    let mut differing_paths = Vec::new();
    if git.outcome == EvaluationOutcome::Safe
        && bonhomme.outcome == EvaluationOutcome::Safe
        && let (Some(tree_oid), Some(semantic_files)) = (&git.tree_oid, semantic_files.as_ref())
    {
        let git_merge_snapshot = TemporaryDirectory::new("shadow-git-merge")?;
        materialize_git_tree(git_dir, tree_oid, &git_merge_snapshot.path)?;
        let git_files = plugin.read_source_tree(&git_merge_snapshot.path)?;
        differing_paths = differing_file_paths(&git_files, semantic_files);
        safe_trees_equal = Some(differing_paths.is_empty());
    }

    Ok(ShadowEvaluation {
        base_commit,
        ours_commit,
        theirs_commit,
        outcomes_agree: git.outcome == bonhomme.outcome,
        semantic_candidate: git.outcome == EvaluationOutcome::Conflict
            && bonhomme.outcome == EvaluationOutcome::Safe,
        git,
        bonhomme,
        safe_trees_equal,
        differing_paths,
    })
}

fn evaluate_git(git_dir: &Path, base: &str, ours: &str, theirs: &str) -> Result<GitEvaluation> {
    let output = git_output(
        git_dir,
        [
            "merge-tree",
            "--write-tree",
            "--messages",
            "--merge-base",
            base,
            ours,
            theirs,
        ],
    )?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let diagnostic = compact_diagnostic(&stdout, &String::from_utf8_lossy(&output.stderr));
    match output.status.code() {
        Some(0) => {
            let tree_oid = stdout
                .lines()
                .next()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .context("git merge-tree succeeded without returning a tree object ID")?;
            Ok(GitEvaluation {
                outcome: EvaluationOutcome::Safe,
                tree_oid: Some(tree_oid.to_string()),
                diagnostic,
            })
        }
        Some(1) => Ok(GitEvaluation {
            outcome: EvaluationOutcome::Conflict,
            tree_oid: None,
            // Git's human message wording changes between versions. Keep golden reports stable;
            // operators can rerun `git merge-tree` directly when they need Git's raw diagnostics.
            diagnostic: Some("git merge-tree reported conflicts".to_string()),
        }),
        code => Ok(GitEvaluation {
            outcome: EvaluationOutcome::Error,
            tree_oid: None,
            diagnostic: Some(format!(
                "git merge-tree exited with {code:?}: {}",
                diagnostic.unwrap_or_else(|| "no diagnostic".to_string())
            )),
        }),
    }
}

async fn evaluate_bonhomme(
    plugin: &dyn LanguagePlugin,
    base_files: &[RenderedFile],
    ours_files: &[RenderedFile],
    theirs_files: &[RenderedFile],
    validate: bool,
) -> (BonhommeEvaluation, Option<Vec<RenderedFile>>) {
    let result: Result<(BonhommeEvaluation, Option<Vec<RenderedFile>>)> = async {
        let base_operations = plugin.import(base_files)?;
        let base_records = operation_records("base", base_operations)?;
        let mut merged_graph = bonhomme_core::materialize(&base_records)?;
        let base_rendered = landed_projection(plugin.render(&merged_graph));
        let base_round_trip_differences = differing_file_paths(base_files, &base_rendered);
        let base_round_trip_equal = base_round_trip_differences.is_empty();
        let mut handler_breakdown = BTreeMap::new();
        let mut blob_paths = Vec::new();
        for symbol in merged_graph
            .root_symbols()
            .into_iter()
            .filter(|symbol| symbol.kind == "file")
        {
            let handler = metadata_string(&symbol.metadata, "handler")
                .unwrap_or_else(|| "untagged".to_string());
            *handler_breakdown.entry(handler.clone()).or_insert(0) += 1;
            if handler == "blob" {
                blob_paths.push(
                    metadata_string(&symbol.metadata, "path")
                        .unwrap_or_else(|| symbol.name.clone()),
                );
            }
        }
        blob_paths.sort();
        let ours_operations = side_operations(plugin, &merged_graph, base_files, ours_files)?;
        let theirs_operations = side_operations(plugin, &merged_graph, base_files, theirs_files)?;
        let ours_records = operation_records("ours", ours_operations)?;
        let theirs_records = operation_records("theirs", theirs_operations)?;
        let analysis = analyze_merge(&ours_records, &theirs_records);
        let conflict_reasons = analysis
            .conflicts
            .iter()
            .map(|conflict| conflict.reason.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if analysis.outcome == MergeOutcome::Conflict {
            return Ok((
                BonhommeEvaluation {
                    outcome: EvaluationOutcome::Conflict,
                    conflicts: analysis.conflicts.len(),
                    conflict_reasons,
                    tree_digest: None,
                    base_round_trip_equal,
                    base_round_trip_differences,
                    handler_breakdown,
                    blob_paths,
                    validation: ValidationResult::NotApplicable,
                    diagnostic: None,
                },
                None,
            ));
        }

        for record in ours_records.iter().chain(&theirs_records) {
            merged_graph.apply_record(record)?;
        }
        merged_graph.validate()?;
        let files = landed_projection(plugin.render(&merged_graph));
        let (validation, diagnostic) = if validate {
            match plugin.validate(&files).await {
                Ok(()) => (ValidationResult::Passed, None),
                Err(error) => (ValidationResult::Failed, Some(format!("{error:#}"))),
            }
        } else {
            (ValidationResult::NotRequested, None)
        };
        let digest = rendered_tree_digest(&files);
        Ok((
            BonhommeEvaluation {
                outcome: EvaluationOutcome::Safe,
                conflicts: 0,
                conflict_reasons,
                tree_digest: Some(digest),
                base_round_trip_equal,
                base_round_trip_differences,
                handler_breakdown,
                blob_paths,
                validation,
                diagnostic,
            },
            Some(files),
        ))
    }
    .await;

    result.unwrap_or_else(|error| {
        (
            BonhommeEvaluation {
                outcome: EvaluationOutcome::Error,
                conflicts: 0,
                conflict_reasons: Vec::new(),
                tree_digest: None,
                base_round_trip_equal: false,
                base_round_trip_differences: Vec::new(),
                handler_breakdown: BTreeMap::new(),
                blob_paths: Vec::new(),
                validation: ValidationResult::NotApplicable,
                diagnostic: Some(format!("{error:#}")),
            },
            None,
        )
    })
}

fn operation_records(label: &str, operations: Vec<Operation>) -> Result<Vec<OperationRecord>> {
    operations
        .into_iter()
        .enumerate()
        .map(|(index, operation)| {
            let payload = serde_json::to_vec(&operation)?;
            let mut identity = format!("shadow:{label}:{index}:").into_bytes();
            identity.extend(payload);
            Ok(OperationRecord {
                id: Uuid::new_v5(&Uuid::NAMESPACE_OID, &identity),
                repository_id: Uuid::nil(),
                branch_id: Uuid::new_v5(&Uuid::NAMESPACE_OID, label.as_bytes()),
                changeset_id: Uuid::new_v5(
                    &Uuid::NAMESPACE_OID,
                    format!("shadow-changeset:{label}").as_bytes(),
                ),
                position: index as i64 + 1,
                operation,
                created_at: DateTime::<Utc>::UNIX_EPOCH,
            })
        })
        .collect()
}

async fn evaluate_corpus(
    plugin: &dyn LanguagePlugin,
    corpus: &Path,
    validate: bool,
    measure_time: bool,
) -> Result<BenchmarkReport> {
    let corpus_started = Instant::now();
    let mut case_directories = fs::read_dir(corpus)
        .with_context(|| format!("could not read benchmark corpus {}", corpus.display()))?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    case_directories.sort();
    if case_directories.is_empty() {
        bail!("benchmark corpus {} contains no cases", corpus.display());
    }

    let mut cases = Vec::new();
    for case_directory in case_directories {
        let case_started = Instant::now();
        let id = case_directory
            .file_name()
            .and_then(OsStr::to_str)
            .context("benchmark case directory name is not UTF-8")?
            .to_string();
        let manifest_path = case_directory.join("case.toml");
        let manifest: BenchmarkManifest = toml::from_str(
            &fs::read_to_string(&manifest_path)
                .with_context(|| format!("could not read {}", manifest_path.display()))?,
        )
        .with_context(|| format!("invalid benchmark manifest {}", manifest_path.display()))?;
        if !matches!(manifest.expected_git.as_str(), "safe" | "conflict") {
            bail!(
                "{}: expectedGit must be safe or conflict",
                manifest_path.display()
            );
        }
        if !matches!(
            manifest.expected_bonhomme.as_str(),
            "safe" | "conflict" | "error"
        ) {
            bail!(
                "{}: expectedBonhomme must be safe, conflict, or error",
                manifest_path.display()
            );
        }
        if let Some(expected_validation) = manifest.expected_validation.as_deref()
            && !matches!(expected_validation, "passed" | "failed" | "not_applicable")
        {
            bail!(
                "{}: expectedValidation must be passed, failed, or not_applicable",
                manifest_path.display()
            );
        }
        if manifest.expected_bonhomme == "safe" && manifest.expected_tree.is_none() {
            bail!(
                "{}: a safe Bonhomme expectation requires expectedTree",
                manifest_path.display()
            );
        }
        let base = fixture_directory(&case_directory, &manifest.base)?;
        let ours = fixture_directory(&case_directory, &manifest.ours)?;
        let theirs = fixture_directory(&case_directory, &manifest.theirs)?;
        let expected = manifest
            .expected_tree
            .as_deref()
            .map(|name| fixture_directory(&case_directory, name))
            .transpose()?;

        let repository = TemporaryDirectory::new("benchmark-repository")?;
        init_git_repository(&repository.path)?;
        let base_commit = commit_fixture(&repository.path, &base, "base", None, 0)?;
        let ours_commit = commit_fixture(&repository.path, &ours, "ours", Some(&base_commit), 1)?;
        let theirs_commit =
            commit_fixture(&repository.path, &theirs, "theirs", Some(&base_commit), 2)?;
        let evaluation = evaluate_revisions(
            plugin,
            &repository.path,
            &base_commit,
            &ours_commit,
            &theirs_commit,
            validate,
        )
        .await?;

        let mut failures = Vec::new();
        compare_expected_outcome(
            "Git",
            &manifest.expected_git,
            &evaluation.git.outcome,
            &mut failures,
        );
        compare_expected_outcome(
            "Bonhomme",
            &manifest.expected_bonhomme,
            &evaluation.bonhomme.outcome,
            &mut failures,
        );
        let mut unsafe_result = false;
        if let Some(expected) = expected {
            let expected_files = plugin.read_source_tree(&expected)?;
            let expected_digest = rendered_tree_digest(&expected_files);
            if evaluation.bonhomme.tree_digest.as_deref() != Some(expected_digest.as_str()) {
                unsafe_result = evaluation.bonhomme.outcome == EvaluationOutcome::Safe;
                failures.push(format!(
                    "Bonhomme tree digest {:?} did not match expected {expected_digest}",
                    evaluation.bonhomme.tree_digest
                ));
            }
        }
        if let Some(expected_round_trip) = manifest.expected_base_round_trip
            && evaluation.bonhomme.base_round_trip_equal != expected_round_trip
        {
            failures.push(format!(
                "base round-trip equality was {}, expected {expected_round_trip}",
                evaluation.bonhomme.base_round_trip_equal
            ));
        }
        let mut expected_blob_paths = manifest.expected_blob_paths;
        expected_blob_paths.sort();
        if evaluation.bonhomme.blob_paths != expected_blob_paths {
            failures.push(format!(
                "blob paths were {:?}, expected {:?}",
                evaluation.bonhomme.blob_paths, expected_blob_paths
            ));
        }
        if validate {
            let expected_validation = manifest.expected_validation.as_deref().unwrap_or(
                if manifest.expected_bonhomme == "safe" {
                    "passed"
                } else {
                    "not_applicable"
                },
            );
            let actual_validation = match evaluation.bonhomme.validation {
                ValidationResult::Passed => "passed",
                ValidationResult::Failed => "failed",
                ValidationResult::NotApplicable => "not_applicable",
                ValidationResult::NotRequested => "not_requested",
            };
            if actual_validation != expected_validation {
                unsafe_result = evaluation.bonhomme.outcome == EvaluationOutcome::Safe
                    && actual_validation == "failed";
                failures.push(format!(
                    "validation was {actual_validation}, expected {expected_validation}"
                ));
            }
        }
        if manifest.expected_bonhomme != "safe"
            && evaluation.bonhomme.outcome == EvaluationOutcome::Safe
        {
            unsafe_result = true;
        }

        cases.push(BenchmarkCaseResult {
            id,
            name: manifest.name,
            description: manifest.description,
            passed: failures.is_empty(),
            failures,
            unsafe_result,
            elapsed_micros: measure_time.then(|| case_started.elapsed().as_micros()),
            evaluation,
        });
    }

    let passed = cases.iter().filter(|case| case.passed).count();
    Ok(BenchmarkReport {
        schema_version: 1,
        corpus: "semantic-merge-v1".to_string(),
        total: cases.len(),
        passed,
        failed: cases.len() - passed,
        git_clean_merges: cases
            .iter()
            .filter(|case| case.evaluation.git.outcome == EvaluationOutcome::Safe)
            .count(),
        git_clean_merge_rate: cases
            .iter()
            .filter(|case| case.evaluation.git.outcome == EvaluationOutcome::Safe)
            .count() as f64
            / cases.len() as f64,
        bonhomme_clean_merges: cases
            .iter()
            .filter(|case| case.evaluation.bonhomme.outcome == EvaluationOutcome::Safe)
            .count(),
        bonhomme_clean_merge_rate: cases
            .iter()
            .filter(|case| case.evaluation.bonhomme.outcome == EvaluationOutcome::Safe)
            .count() as f64
            / cases.len() as f64,
        git_conflicts: cases
            .iter()
            .filter(|case| case.evaluation.git.outcome == EvaluationOutcome::Conflict)
            .count(),
        bonhomme_conflicts: cases
            .iter()
            .filter(|case| case.evaluation.bonhomme.outcome == EvaluationOutcome::Conflict)
            .count(),
        bonhomme_conflict_records: cases
            .iter()
            .map(|case| case.evaluation.bonhomme.conflicts)
            .sum(),
        semantic_candidates: cases
            .iter()
            .filter(|case| case.evaluation.semantic_candidate)
            .count(),
        unsafe_results: cases.iter().filter(|case| case.unsafe_result).count(),
        incorrect_results: cases.iter().filter(|case| !case.passed).count(),
        timing_mode: if measure_time {
            "wall_clock"
        } else {
            "suppressed"
        },
        elapsed_micros: measure_time.then(|| corpus_started.elapsed().as_micros()),
        cases,
    })
}

fn side_operations(
    plugin: &dyn LanguagePlugin,
    base_graph: &bonhomme_core::SemanticGraph,
    base_files: &[RenderedFile],
    modified_files: &[RenderedFile],
) -> Result<Vec<Operation>> {
    if base_files == modified_files {
        return Ok(Vec::new());
    }
    let modified_paths = modified_files
        .iter()
        .map(|file| file.path.as_str())
        .collect::<BTreeSet<_>>();
    let deleted_paths = base_files
        .iter()
        .map(|file| file.path.as_str())
        .filter(|path| !modified_paths.contains(path))
        .collect::<Vec<_>>();
    let deletes = super::commands::delete_file_operations(base_graph, deleted_paths);
    let recovered = plugin.diff(base_files, modified_files)?;
    Ok(detect_moves(
        super::commands::merge_delete_and_recovered_operations(deletes, recovered),
        base_graph,
    ))
}

fn compare_expected_outcome(
    label: &str,
    expected: &str,
    actual: &EvaluationOutcome,
    failures: &mut Vec<String>,
) {
    let actual = match actual {
        EvaluationOutcome::Safe => "safe",
        EvaluationOutcome::Conflict => "conflict",
        EvaluationOutcome::Error => "error",
    };
    if expected != actual {
        failures.push(format!("{label} outcome was {actual}, expected {expected}"));
    }
}

fn fixture_directory(case_directory: &Path, name: &str) -> Result<PathBuf> {
    let mut components = Path::new(name).components();
    let valid = matches!(components.next(), Some(Component::Normal(_)))
        && components.next().is_none()
        && name != ".git";
    if !valid {
        bail!("benchmark fixture must be one safe child directory: {name}");
    }
    let path = case_directory.join(name);
    if !path.is_dir() {
        bail!("benchmark fixture {} is not a directory", path.display());
    }
    Ok(path)
}

fn commit_fixture(
    repository: &Path,
    fixture: &Path,
    label: &str,
    parent: Option<&str>,
    seconds: u32,
) -> Result<String> {
    clear_worktree(repository)?;
    copy_tree(fixture, repository)?;
    run_git_checked(repository, ["add", "--force", "--all", "--", "."])?;
    let tree = git_stdout(repository, ["write-tree"])?;
    let mut command = git_command(repository);
    command.args(["commit-tree", &tree]);
    if let Some(parent) = parent {
        command.args(["-p", parent]);
    }
    command.args(["-m", label]);
    let timestamp = format!("2000-01-01T00:00:{seconds:02}Z");
    command
        .env("GIT_AUTHOR_NAME", GIT_AUTHOR_NAME)
        .env("GIT_AUTHOR_EMAIL", GIT_AUTHOR_EMAIL)
        .env("GIT_COMMITTER_NAME", GIT_AUTHOR_NAME)
        .env("GIT_COMMITTER_EMAIL", GIT_AUTHOR_EMAIL)
        .env("GIT_AUTHOR_DATE", &timestamp)
        .env("GIT_COMMITTER_DATE", &timestamp);
    checked_stdout(command, "git commit-tree")
}

fn prepare_empty_destination(path: &Path) -> Result<()> {
    if path.exists() {
        if !path.is_dir() {
            bail!("export destination {} is not a directory", path.display());
        }
        if fs::read_dir(path)?.next().is_some() {
            bail!(
                "export destination {} is not empty; refusing to overwrite it",
                path.display()
            );
        }
    } else {
        fs::create_dir_all(path)
            .with_context(|| format!("could not create export destination {}", path.display()))?;
    }
    Ok(())
}

fn write_rendered_files(root: &Path, files: &[RenderedFile]) -> Result<()> {
    for file in files {
        let destination = root.join(safe_relative_path(&file.path)?);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        match decode_binary(&file.content) {
            Some(bytes) => fs::write(&destination, bytes)?,
            None => fs::write(&destination, file.content.as_bytes())?,
        }
    }
    Ok(())
}

/// Apply the non-destructive part of the normal repository write-back boundary. Git exports and
/// shadow reports must describe the bytes that would land in Git, not Bonhomme's internal
/// generated-file marker. Project-specific formatters remain an explicit deployment choice.
fn landed_projection(mut files: Vec<RenderedFile>) -> Vec<RenderedFile> {
    for file in &mut files {
        if decode_binary(&file.content).is_none() {
            file.content = crate::format::strip_generated_banner(&file.content);
        }
    }
    files
}

fn materialize_git_tree(git_dir: &Path, treeish: &str, destination: &Path) -> Result<()> {
    let tree = git_stdout(git_dir, ["rev-parse", "--verify", treeish])?;
    let output = git_output(
        git_dir,
        ["ls-tree", "-r", "-z", "--full-tree", tree.as_str()],
    )?;
    ensure_success(&output, "git ls-tree")?;
    for raw_entry in output.stdout.split(|byte| *byte == 0) {
        if raw_entry.is_empty() {
            continue;
        }
        let entry = std::str::from_utf8(raw_entry).context("Git tree entry is not UTF-8")?;
        let (metadata, path) = entry
            .split_once('\t')
            .context("malformed git ls-tree entry")?;
        let mut metadata = metadata.split_whitespace();
        let mode = metadata.next().context("Git tree entry has no mode")?;
        let kind = metadata
            .next()
            .context("Git tree entry has no object type")?;
        let oid = metadata.next().context("Git tree entry has no object ID")?;
        if kind != "blob" || mode == "160000" {
            bail!("Git tree contains unsupported {kind} entry {path} (mode {mode})");
        }
        if mode == "120000" {
            bail!("Git tree contains symlink {path}; semantic snapshots do not follow symlinks");
        }
        if mode != "100644" {
            bail!(
                "Git tree entry {path} has mode {mode}; the semantic file model currently supports only regular non-executable files (100644)"
            );
        }
        let relative = safe_relative_path(path)?;
        let destination = destination.join(relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        let blob = git_output(git_dir, ["cat-file", "blob", oid])?;
        ensure_success(&blob, "git cat-file")?;
        fs::write(&destination, blob.stdout)?;
    }
    Ok(())
}

fn resolve_commit(git_dir: &Path, revision: &str) -> Result<String> {
    git_stdout(
        git_dir,
        [
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{revision}^{{commit}}"),
        ],
    )
    .with_context(|| format!("could not resolve Git commit {revision}"))
}

fn rendered_tree_digest(files: &[RenderedFile]) -> String {
    let mut files = files.iter().collect::<Vec<_>>();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let mut digest = Sha256::new();
    for file in files {
        digest.update((file.path.len() as u64).to_be_bytes());
        digest.update(file.path.as_bytes());
        let bytes =
            decode_binary(&file.content).unwrap_or_else(|| file.content.as_bytes().to_vec());
        digest.update((bytes.len() as u64).to_be_bytes());
        digest.update(bytes);
    }
    format!("sha256:{:x}", digest.finalize())
}

fn differing_file_paths(left: &[RenderedFile], right: &[RenderedFile]) -> Vec<String> {
    let left = file_map(left);
    let right = file_map(right);
    left.keys()
        .chain(right.keys())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|path| left.get(*path) != right.get(*path))
        .cloned()
        .collect()
}

fn file_map(files: &[RenderedFile]) -> BTreeMap<String, Vec<u8>> {
    files
        .iter()
        .map(|file| {
            (
                file.path.clone(),
                decode_binary(&file.content).unwrap_or_else(|| file.content.as_bytes().to_vec()),
            )
        })
        .collect()
}

fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let target = destination.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            fs::create_dir_all(&target)?;
            copy_tree(&entry.path(), &target)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), target)?;
        } else {
            bail!(
                "benchmark fixture contains unsupported non-file entry {}",
                entry.path().display()
            );
        }
    }
    Ok(())
}

fn clear_worktree(repository: &Path) -> Result<()> {
    for entry in fs::read_dir(repository)? {
        let entry = entry?;
        if entry.file_name() == OsStr::new(".git") {
            continue;
        }
        if entry.file_type()?.is_dir() {
            fs::remove_dir_all(entry.path())?;
        } else {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

fn compact_diagnostic(stdout: &str, stderr: &str) -> Option<String> {
    let combined = [stdout, stderr]
        .into_iter()
        .flat_map(str::lines)
        .skip(1)
        .take(40)
        .collect::<Vec<_>>()
        .join("\n");
    (!combined.trim().is_empty()).then(|| combined.trim().to_string())
}

fn init_git_repository(repository: &Path) -> Result<()> {
    run_git_checked(
        repository,
        [
            "init",
            "--quiet",
            "--object-format=sha1",
            "--initial-branch=main",
        ],
    )
}

fn isolated_git_home() -> &'static Path {
    ISOLATED_GIT_HOME
        .get_or_init(|| {
            let home =
                std::env::temp_dir().join(format!("bonhomme-git-environment-{}", Uuid::new_v4()));
            // The command will report a useful failure if the environment cannot create these.
            // Keeping the directory for the process lifetime also avoids per-subprocess races.
            let _ = fs::create_dir_all(home.join("hooks"));
            let _ = fs::write(home.join("attributes"), []);
            home
        })
        .as_path()
}

fn git_command(repository: &Path) -> Command {
    let isolated_home = isolated_git_home();
    let mut command = Command::new("git");
    command
        .arg("--no-pager")
        .arg("--no-replace-objects")
        .args(["-c", "core.autocrlf=false"])
        .args(["-c", "core.safecrlf=false"])
        .arg("-c")
        .arg(format!(
            "core.hooksPath={}",
            isolated_home.join("hooks").display()
        ))
        .arg("-c")
        .arg(format!(
            "core.attributesFile={}",
            isolated_home.join("attributes").display()
        ))
        .args(["-c", "filter.lfs.required=false"])
        .args(["-c", "filter.lfs.clean="])
        .args(["-c", "filter.lfs.smudge="])
        .args(["-c", "filter.lfs.process="])
        .arg("-C")
        .arg(repository)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_ATTR_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", isolated_home.join("config"))
        .env("HOME", isolated_home)
        .env("XDG_CONFIG_HOME", isolated_home)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_LFS_SKIP_SMUDGE", "1");
    for key in [
        "GIT_CONFIG_COUNT",
        "GIT_CONFIG_PARAMETERS",
        "GIT_CONFIG_SYSTEM",
        "GIT_CONFIG",
        "GIT_ATTR_SOURCE",
        "GIT_EXEC_PATH",
        "GIT_TEMPLATE_DIR",
        "GIT_DEFAULT_HASH",
        "GIT_DEFAULT_REF_FORMAT",
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_COMMON_DIR",
        "GIT_NAMESPACE",
        "GIT_REPLACE_REF_BASE",
        "GIT_EXTERNAL_DIFF",
        "GIT_DIFF_OPTS",
        "GIT_SSH",
        "GIT_SSH_COMMAND",
        "GIT_ASKPASS",
    ] {
        command.env_remove(key);
    }
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_CONFIG_KEY_")
            || key.to_string_lossy().starts_with("GIT_CONFIG_VALUE_")
        {
            command.env_remove(key);
        }
    }
    command
}

fn git_output<I, S>(repository: &Path, args: I) -> Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    git_command(repository)
        .args(args)
        .output()
        .with_context(|| format!("could not run git in {}", repository.display()))
}

fn run_git_checked<I, S>(repository: &Path, args: I) -> Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = git_output(repository, args)?;
    ensure_success(&output, "git command")
}

fn git_stdout<I, S>(repository: &Path, args: I) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = git_output(repository, args)?;
    ensure_success(&output, "git command")?;
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

fn checked_stdout(mut command: Command, description: &str) -> Result<String> {
    let output = command
        .output()
        .with_context(|| format!("could not run {description}"))?;
    ensure_success(&output, description)?;
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

fn ensure_success(output: &Output, description: &str) -> Result<()> {
    if output.status.success() {
        return Ok(());
    }
    bail!(
        "{description} failed with {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENVIRONMENT_LOCK: Mutex<()> = Mutex::new(());

    struct EnvironmentRestore {
        key: &'static str,
        value: Option<std::ffi::OsString>,
    }

    impl EnvironmentRestore {
        fn set(key: &'static str, value: &Path) -> Self {
            let previous = std::env::var_os(key);
            // SAFETY: this test serializes its environment mutation and the production Git wrapper
            // overrides the variable for every child process. The guard restores it on unwind.
            unsafe { std::env::set_var(key, value) };
            Self {
                key,
                value: previous,
            }
        }
    }

    impl Drop for EnvironmentRestore {
        fn drop(&mut self) {
            // SAFETY: paired with the serialized mutation in `EnvironmentRestore::set`.
            unsafe {
                match &self.value {
                    Some(value) => std::env::set_var(self.key, value),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

    fn write(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    #[test]
    fn git_snapshot_reads_an_immutable_commit_without_checkout() {
        let repository = TemporaryDirectory::new("git-snapshot-test").unwrap();
        init_git_repository(&repository.path).unwrap();
        write(
            &repository.path.join("src/lib.rs"),
            "pub fn value() -> u8 { 1 }\n",
        );
        run_git_checked(&repository.path, ["add", "--force", "--all", "--", "."]).unwrap();
        let tree = git_stdout(&repository.path, ["write-tree"]).unwrap();
        let mut command = git_command(&repository.path);
        command
            .args(["commit-tree", &tree, "-m", "snapshot"])
            .env("GIT_AUTHOR_NAME", GIT_AUTHOR_NAME)
            .env("GIT_AUTHOR_EMAIL", GIT_AUTHOR_EMAIL)
            .env("GIT_COMMITTER_NAME", GIT_AUTHOR_NAME)
            .env("GIT_COMMITTER_EMAIL", GIT_AUTHOR_EMAIL);
        let commit = checked_stdout(command, "git commit-tree").unwrap();
        write(&repository.path.join("src/lib.rs"), "uncommitted\n");

        let snapshot = TemporaryDirectory::new("git-snapshot-output").unwrap();
        materialize_git_tree(
            &repository.path,
            &format!("{commit}^{{tree}}"),
            &snapshot.path,
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(snapshot.path.join("src/lib.rs")).unwrap(),
            "pub fn value() -> u8 { 1 }\n"
        );
        assert_eq!(
            fs::read_to_string(repository.path.join("src/lib.rs")).unwrap(),
            "uncommitted\n"
        );
    }

    #[test]
    fn tree_digest_is_order_independent_and_content_sensitive() {
        let first = vec![
            RenderedFile {
                path: "b.txt".to_string(),
                content: "two".to_string(),
            },
            RenderedFile {
                path: "a.txt".to_string(),
                content: "one".to_string(),
            },
        ];
        let mut reordered = first.clone();
        reordered.reverse();
        assert_eq!(
            rendered_tree_digest(&first),
            rendered_tree_digest(&reordered)
        );
        reordered[0].content.push('!');
        assert_ne!(
            rendered_tree_digest(&first),
            rendered_tree_digest(&reordered)
        );
    }

    #[test]
    fn exported_git_tree_id_is_deterministic() {
        let files = vec![
            RenderedFile {
                path: "src/lib.rs".to_string(),
                content: "pub fn value() -> u8 { 1 }\n".to_string(),
            },
            RenderedFile {
                path: ".gitignore".to_string(),
                content: "generated.txt\n".to_string(),
            },
            // Force-add proves a tracked path is not lost merely because the exported snapshot's
            // own ignore policy matches it.
            RenderedFile {
                path: "generated.txt".to_string(),
                content: "tracked\n".to_string(),
            },
        ];
        let export = || {
            let directory = TemporaryDirectory::new("git-export-test").unwrap();
            write_rendered_files(&directory.path, &files).unwrap();
            init_git_repository(&directory.path).unwrap();
            run_git_checked(&directory.path, ["add", "--force", "--all", "--", "."]).unwrap();
            let tree = git_stdout(&directory.path, ["write-tree"]).unwrap();
            assert!(
                git_stdout(&directory.path, ["ls-tree", "-r", "--name-only", &tree])
                    .unwrap()
                    .lines()
                    .any(|path| path == "generated.txt")
            );
            tree
        };
        assert_eq!(export(), export());
    }

    #[test]
    fn git_commands_ignore_hostile_global_config_and_preserve_bytes() {
        let _environment = ENVIRONMENT_LOCK.lock().unwrap();
        let hostile_home = TemporaryDirectory::new("git-hostile-config").unwrap();
        let hostile_config = hostile_home.path.join("config");
        write(
            &hostile_config,
            concat!(
                "[init]\n",
                "  defaultObjectFormat = sha256\n",
                "  defaultBranch = poisoned\n",
                "[core]\n",
                "  autocrlf = true\n",
                "[filter \"poison\"]\n",
                "  clean = command-that-must-not-run\n",
                "  required = true\n",
            ),
        );
        let _restore = EnvironmentRestore::set("GIT_CONFIG_GLOBAL", &hostile_config);

        let repository = TemporaryDirectory::new("git-isolation-test").unwrap();
        init_git_repository(&repository.path).unwrap();
        write(
            &repository.path.join(".gitattributes"),
            "payload.txt filter=poison\n",
        );
        write(
            &repository.path.join("payload.txt"),
            "line one\r\nline two\r\n",
        );
        run_git_checked(&repository.path, ["add", "--force", "--all", "--", "."]).unwrap();
        let tree = git_stdout(&repository.path, ["write-tree"]).unwrap();
        let blob_oid = git_stdout(
            &repository.path,
            ["rev-parse", "--verify", &format!("{tree}:payload.txt")],
        )
        .unwrap();
        let blob = git_output(&repository.path, ["cat-file", "blob", &blob_oid]).unwrap();
        ensure_success(&blob, "git cat-file").unwrap();

        assert_eq!(
            git_stdout(&repository.path, ["rev-parse", "--show-object-format"]).unwrap(),
            "sha1"
        );
        assert_eq!(blob.stdout, b"line one\r\nline two\r\n");
    }

    #[test]
    fn git_export_manifest_is_a_deterministic_context_bound_operation_batch() {
        let repository_id = Uuid::from_u128(2);
        let branch_id = Uuid::from_u128(3);
        let records = vec![OperationRecord {
            id: Uuid::from_u128(4),
            repository_id,
            branch_id,
            changeset_id: Uuid::from_u128(5),
            position: 1,
            operation: Operation::DeleteReference {
                reference_id: Uuid::from_u128(6),
            },
            created_at: DateTime::<Utc>::UNIX_EPOCH,
        }];
        let first = export_operation_batch(repository_id, branch_id, "main", &records).unwrap();
        let second = export_operation_batch(repository_id, branch_id, "main", &records).unwrap();
        assert_eq!(first, second);
        first.validate_context(repository_id, branch_id, 0).unwrap();
        assert_eq!(
            serde_json::to_value(first).unwrap()["protocolVersion"],
            serde_json::json!({"major": 1, "minor": 0})
        );
    }

    #[test]
    fn fixture_names_cannot_escape_the_case_directory() {
        let case = Path::new("case");
        assert!(fixture_directory(case, "../outside").is_err());
        assert!(fixture_directory(case, "/outside").is_err());
        assert!(fixture_directory(case, ".git").is_err());
    }

    #[tokio::test]
    async fn semantic_shadow_combines_independent_adjacent_functions() {
        let file = |content: &str| {
            vec![RenderedFile {
                path: "src/pricing.ts".to_string(),
                content: content.to_string(),
            }]
        };
        let base = file(concat!(
            "export function shipping(): number { return 5; }\n",
            "export function tax(): number { return 10; }\n"
        ));
        let ours = file(concat!(
            "export function shipping(): number { return 7; }\n",
            "export function tax(): number { return 10; }\n"
        ));
        let theirs = file(concat!(
            "export function shipping(): number { return 5; }\n",
            "export function tax(): number { return 12; }\n"
        ));
        let (evaluation, rendered) = evaluate_bonhomme(
            &bonhomme_ts::TypeScriptPlugin::default(),
            &base,
            &ours,
            &theirs,
            false,
        )
        .await;
        assert_eq!(evaluation.outcome, EvaluationOutcome::Safe);
        assert_eq!(
            rendered.unwrap()[0].content,
            concat!(
                "export function shipping(): number {\n",
                "  return 7;\n",
                "}\n\n",
                "export function tax(): number {\n",
                "  return 12;\n",
                "}\n\n",
            )
        );
    }

    #[tokio::test]
    async fn shadow_evaluation_does_not_move_git_refs() {
        let repository = TemporaryDirectory::new("git-shadow-ref-test").unwrap();
        let base = TemporaryDirectory::new("git-shadow-base-fixture").unwrap();
        let ours = TemporaryDirectory::new("git-shadow-ours-fixture").unwrap();
        let theirs = TemporaryDirectory::new("git-shadow-theirs-fixture").unwrap();
        write(&base.path.join("left.opaque"), "base\n");
        write(&ours.path.join("left.opaque"), "ours\n");
        write(&theirs.path.join("left.opaque"), "base\n");
        write(&theirs.path.join("right.opaque"), "theirs\n");
        init_git_repository(&repository.path).unwrap();
        let base_commit = commit_fixture(&repository.path, &base.path, "base", None, 0).unwrap();
        let ours_commit =
            commit_fixture(&repository.path, &ours.path, "ours", Some(&base_commit), 1).unwrap();
        let theirs_commit = commit_fixture(
            &repository.path,
            &theirs.path,
            "theirs",
            Some(&base_commit),
            2,
        )
        .unwrap();
        for (name, oid) in [
            ("base", base_commit.as_str()),
            ("ours", ours_commit.as_str()),
            ("theirs", theirs_commit.as_str()),
        ] {
            run_git_checked(
                &repository.path,
                ["update-ref", &format!("refs/heads/{name}"), oid],
            )
            .unwrap();
        }
        let refs_before = git_stdout(
            &repository.path,
            ["for-each-ref", "--format=%(refname) %(objectname)"],
        )
        .unwrap();
        let evaluation = evaluate_revisions(
            &bonhomme_core::BlobHandler,
            &repository.path,
            "base",
            "ours",
            "theirs",
            false,
        )
        .await
        .unwrap();
        assert_eq!(evaluation.git.outcome, EvaluationOutcome::Safe);
        assert_eq!(evaluation.bonhomme.outcome, EvaluationOutcome::Safe);
        assert_eq!(
            refs_before,
            git_stdout(
                &repository.path,
                ["for-each-ref", "--format=%(refname) %(objectname)"],
            )
            .unwrap()
        );
    }
}
