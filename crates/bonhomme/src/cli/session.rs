//! Coauth-session scaffold: `session start` imports a working tree into a local
//! `.bonhomme/session.db`, `session review` opens the explorer over it, and `session land` renders
//! changed paths back out from the recorded base position. `session check` remains the
//! side-effect-free round-trip fidelity gate. See `reports/coauth-session-plan.md`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use bonhomme_core::{
    LanguagePlugin, Operation, OperationRecord, RenderedFile, SemanticGraph, SymbolNode,
    decode_binary, metadata_string,
};
use bonhomme_engine::Storage;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::fs;
use uuid::Uuid;

use super::SessionCommand;
use crate::{config::Config, explorer};

const SESSION_DB_NAME: &str = "session.db";
const SESSION_MANIFEST_NAME: &str = "session.json";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionManifest {
    version: u32,
    database_url: String,
    repository: String,
    branch: String,
    base_position: i64,
    root: String,
    source_path: String,
}

/// The outcome of importing a tree and rendering it back, bucketed by fidelity. A diff-clean
/// round-trip — the prerequisite for trustworthy write-back — has empty `reformatted`/`dropped`/
/// `added`.
#[derive(Debug, Default)]
pub struct RoundtripReport {
    /// Rendered byte-for-byte identical to the source — what write-back needs.
    pub clean: Vec<String>,
    /// Round-tripped to the same path but with different bytes (the formatter-alignment gap).
    pub reformatted: Vec<String>,
    /// Present in the source but missing from the render (a fidelity bug, never acceptable).
    pub dropped: Vec<String>,
    /// Produced by the render but absent from the source (the renderer invented a file).
    pub added: Vec<String>,
}

impl RoundtripReport {
    fn compare(source: Vec<RenderedFile>, rendered: Vec<RenderedFile>) -> Self {
        let original: BTreeMap<String, String> =
            source.into_iter().map(|f| (f.path, f.content)).collect();
        let mut rendered: BTreeMap<String, String> =
            rendered.into_iter().map(|f| (f.path, f.content)).collect();

        let mut report = RoundtripReport::default();
        for (path, content) in original {
            match rendered.remove(&path) {
                Some(out) if out == content => report.clean.push(path),
                Some(_) => report.reformatted.push(path),
                None => report.dropped.push(path),
            }
        }
        report.added = rendered.into_keys().collect();
        report
    }

    pub fn is_clean(&self) -> bool {
        self.reformatted.is_empty() && self.dropped.is_empty() && self.added.is_empty()
    }

    fn print(&self, root: &Path) {
        println!("round-trip check: {}", root.display());
        println!(
            "  {} clean, {} reformatted, {} dropped, {} added",
            self.clean.len(),
            self.reformatted.len(),
            self.dropped.len(),
            self.added.len()
        );
        for (label, paths) in [
            ("reformatted", &self.reformatted),
            ("dropped", &self.dropped),
            ("added", &self.added),
        ] {
            if !paths.is_empty() {
                println!("  {label}:");
                for path in paths {
                    println!("    {path}");
                }
            }
        }
    }
}

fn session_dir(root: &Path) -> PathBuf {
    root.join(".bonhomme")
}

fn session_database_url(root: &Path) -> String {
    format!(
        "turso:{}",
        session_dir(root).join(SESSION_DB_NAME).display()
    )
}

fn manifest_path(root: &Path) -> PathBuf {
    session_dir(root).join(SESSION_MANIFEST_NAME)
}

async fn read_session_manifest(root: &Path) -> Result<Option<SessionManifest>> {
    let path = manifest_path(root);
    match fs::read_to_string(&path).await {
        Ok(text) => Ok(Some(
            serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?,
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
}

async fn write_session_manifest(root: &Path, manifest: &SessionManifest) -> Result<()> {
    let path = manifest_path(root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await?;
    }
    fs::write(&path, serde_json::to_vec_pretty(manifest)?)
        .await
        .with_context(|| format!("writing {}", path.display()))
}

fn default_repository_name(root: &Path) -> Result<String> {
    root.file_name()
        .and_then(|name| name.to_str())
        .map(ToOwned::to_owned)
        .with_context(|| format!("could not infer repository name from {}", root.display()))
}

fn resolve_session_database_url(
    root: &Path,
    manifest: Option<&SessionManifest>,
    explicit_database_url: Option<&str>,
) -> String {
    explicit_database_url
        .map(ToOwned::to_owned)
        .or_else(|| manifest.map(|manifest| manifest.database_url.clone()))
        .unwrap_or_else(|| session_database_url(root))
}

fn resolve_session_ref(
    root: &Path,
    manifest: Option<&SessionManifest>,
    repo: Option<String>,
    branch: Option<String>,
) -> Result<(String, String)> {
    let repository = repo
        .or_else(|| manifest.map(|manifest| manifest.repository.clone()))
        .map(Ok)
        .unwrap_or_else(|| default_repository_name(root))?;
    let branch = branch
        .or_else(|| manifest.map(|manifest| manifest.branch.clone()))
        .unwrap_or_else(|| "main".to_string());
    Ok((repository, branch))
}

/// Import `src` into a throwaway in-memory session and render it straight back, reporting how
/// faithfully each file reproduces. Side-effect-free: it touches neither the configured database
/// nor the disk.
pub async fn check(
    plugin: Arc<dyn LanguagePlugin>,
    src: &Path,
    formatters: &BTreeMap<String, String>,
) -> Result<RoundtripReport> {
    let storage = Storage::connect(":memory:", plugin).await?;
    storage.migrate().await?;
    let (repository, branch) = storage.init_repository("roundtrip").await?;

    let source_files = storage.plugin().read_source_tree(src)?;
    let operations = storage.plugin().import(&source_files)?;

    let task = storage
        .create_task(repository.id, "round-trip check")
        .await?;
    let changeset = storage
        .create_changeset(
            repository.id,
            task.id,
            branch.id,
            "round-trip",
            "bonhomme-roundtrip",
        )
        .await?;
    for operation in operations {
        storage
            .append_operation(repository.id, branch.id, changeset.id, operation)
            .await?;
    }

    let materialized = storage
        .materialize_branch("roundtrip", &branch.name)
        .await?;
    // Format the rendered side exactly as `land` would, so the gate measures what landing actually
    // writes against the on-disk source rather than flagging the repo's own formatting.
    let rendered = crate::format::format_files(materialized.files, formatters).await;
    Ok(RoundtripReport::compare(source_files, rendered))
}

async fn repository_exists(storage: &Storage, name: &str) -> Result<bool> {
    match storage.repository_by_name(name).await {
        Ok(_) => Ok(true),
        Err(error) => {
            let message = format!("{:#}", error);
            if message.contains("does not exist") {
                Ok(false)
            } else {
                Err(error)
            }
        }
    }
}

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

async fn start_session(
    config: &Config,
    root: &Path,
    explicit_database_url: Option<&str>,
    path: Option<PathBuf>,
    repo: Option<String>,
    branch_name: String,
    reset: bool,
    no_validate: bool,
) -> Result<()> {
    let source_path = path.unwrap_or_else(|| root.to_path_buf());
    let repository_name = repo
        .map(Ok)
        .unwrap_or_else(|| default_repository_name(root))?;
    let database_url = explicit_database_url
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| session_database_url(root));
    let storage =
        Storage::connect(&database_url, crate::plugins::language_registry(config)).await?;
    storage.migrate().await?;

    let (repository, main) = if reset {
        storage.reset_repository(&repository_name).await?
    } else if repository_exists(&storage, &repository_name).await? {
        bail!(
            "session repository {repository_name} already exists in {}; pass --reset to replace it",
            database_url
        );
    } else {
        storage.init_repository(&repository_name).await?
    };
    let branch = if branch_name == "main" {
        main
    } else {
        storage
            .create_branch(repository.id, &branch_name, "main")
            .await?
    };

    let files = storage.plugin().read_source_tree(&source_path)?;
    let operations = storage.plugin().import(&files)?;
    let task = storage
        .create_task(
            repository.id,
            &format!("Start coauth session from {}", source_path.display()),
        )
        .await?;
    let changeset = storage
        .create_changeset(
            repository.id,
            task.id,
            branch.id,
            "Start coauth session",
            "bonhomme-session",
        )
        .await?;
    storage
        .add_attachment(
            repository.id,
            "task",
            task.id,
            "CoauthSessionAttachment",
            json!({
                "root": root,
                "sourcePath": source_path,
                "databaseUrl": database_url,
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

    let materialized = storage
        .materialize_branch(&repository_name, &branch.name)
        .await?;
    materialized.graph.validate()?;
    if !no_validate {
        storage.plugin().validate(&materialized.files).await?;
    }

    let manifest = SessionManifest {
        version: 1,
        database_url: database_url.clone(),
        repository: repository_name.clone(),
        branch: branch.name.clone(),
        base_position: materialized.operations.len() as i64,
        root: root.display().to_string(),
        source_path: source_path.display().to_string(),
    };
    write_session_manifest(root, &manifest).await?;

    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "session": manifest,
            "filesImported": files.len(),
            "operationsAppended": appended.len(),
            "symbols": materialized.graph.symbols.len(),
            "references": materialized.graph.references.len(),
            "handlerBreakdown": handler_breakdown(&materialized.graph),
            "validated": !no_validate
        }))?
    );
    Ok(())
}

/// Files written and removed by a `land`.
pub struct LandStats {
    pub written: usize,
    pub deleted: usize,
}

/// Where the per-destination manifest of bonhomme-written paths lives. Kept under `.bonhomme/`
/// (gitignored), so it never shows up in the diff it exists to keep clean.
fn land_manifest_path(dest: &Path) -> std::path::PathBuf {
    dest.join(".bonhomme").join("land-manifest.json")
}

async fn read_manifest(dest: &Path) -> Vec<String> {
    match fs::read_to_string(land_manifest_path(dest)).await {
        Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

async fn write_land_manifest(dest: &Path, paths: &BTreeSet<String>) -> Result<()> {
    let manifest = land_manifest_path(dest);
    if let Some(parent) = manifest.parent() {
        fs::create_dir_all(parent).await?;
    }
    let paths: Vec<&String> = paths.iter().collect();
    fs::write(&manifest, serde_json::to_vec_pretty(&paths)?).await?;
    Ok(())
}

async fn write_rendered_file(dest: &Path, file: &RenderedFile) -> Result<()> {
    let path = dest.join(&file.path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await?;
    }
    match decode_binary(&file.content) {
        Some(bytes) => fs::write(&path, bytes).await?,
        None => fs::write(&path, &file.content).await?,
    }
    Ok(())
}

/// Render `files` into `dest`, and — crucially for diff-clean write-back — delete any path bonhomme
/// wrote on a previous `land` that this render no longer produces. Without this, a moved or deleted
/// symbol's old file lingers on disk and the Git diff shows a stale duplicate instead of a clean
/// rename/delete. Only paths recorded in bonhomme's own manifest are ever removed, so a user's
/// unmanaged files are never touched. The formatter boundary (a per-file pass driven by `[format]`
/// config) belongs here, between materialization and the write; it is not wired yet, so `check`
/// reports such drift as `reformatted`.
#[cfg(test)]
async fn land_tree(files: &[RenderedFile], dest: &Path) -> Result<LandStats> {
    let prior: BTreeSet<String> = read_manifest(dest).await.into_iter().collect();
    let current: BTreeSet<String> = files.iter().map(|f| f.path.clone()).collect();

    let mut deleted = 0;
    for orphan in prior.difference(&current) {
        let path = dest.join(orphan);
        if fs::remove_file(&path).await.is_ok() {
            deleted += 1;
            prune_empty_parents(dest, &path).await;
        }
    }

    for file in files {
        write_rendered_file(dest, file).await?;
    }

    write_land_manifest(dest, &current).await?;

    Ok(LandStats {
        written: files.len(),
        deleted,
    })
}

/// Land only the paths touched by operations since `session start`. Paths that still render are
/// written; touched paths absent from the latest render are removed from disk. The persistent land
/// manifest is updated as a union so future lands can still clean up files bonhomme wrote earlier.
async fn land_changed_files(
    files: &[RenderedFile],
    dest: &Path,
    changed_paths: &BTreeSet<String>,
) -> Result<LandStats> {
    let rendered = files
        .iter()
        .map(|file| (file.path.clone(), file))
        .collect::<BTreeMap<_, _>>();
    let mut manifest_paths = read_manifest(dest)
        .await
        .into_iter()
        .collect::<BTreeSet<_>>();

    let mut written = 0;
    let mut deleted = 0;
    for path in changed_paths {
        if let Some(file) = rendered.get(path) {
            write_rendered_file(dest, file).await?;
            manifest_paths.insert(path.clone());
            written += 1;
            continue;
        }

        let disk_path = dest.join(path);
        if fs::remove_file(&disk_path).await.is_ok() {
            deleted += 1;
            prune_empty_parents(dest, &disk_path).await;
        }
        manifest_paths.remove(path);
    }

    write_land_manifest(dest, &manifest_paths).await?;
    Ok(LandStats { written, deleted })
}

/// After deleting an orphan, remove now-empty parent directories up to (but not including) `dest`,
/// so a moved-away file does not leave an empty tree behind. Best-effort: stops at the first
/// non-empty directory.
async fn prune_empty_parents(dest: &Path, file: &Path) {
    let mut dir = file.parent().map(Path::to_path_buf);
    while let Some(current) = dir {
        if current == dest || !current.starts_with(dest) {
            break;
        }
        if fs::remove_dir(&current).await.is_err() {
            break; // non-empty (or gone) — stop walking up
        }
        dir = current.parent().map(Path::to_path_buf);
    }
}

fn changed_file_paths_since(
    base: &SemanticGraph,
    latest: &SemanticGraph,
    operations: &[OperationRecord],
) -> BTreeSet<String> {
    let mut paths = BTreeSet::new();
    for record in operations {
        match &record.operation {
            Operation::CreateSymbol { symbol_id, .. }
            | Operation::DeleteSymbol { symbol_id }
            | Operation::UpdateSymbol { symbol_id, .. }
            | Operation::MoveSymbol { symbol_id, .. } => {
                insert_symbol_paths(base, latest, *symbol_id, &mut paths);
            }
            Operation::CreateReference {
                from_symbol_id,
                to_symbol_id,
                ..
            } => {
                insert_symbol_paths(base, latest, *from_symbol_id, &mut paths);
                insert_symbol_paths(base, latest, *to_symbol_id, &mut paths);
            }
            Operation::DeleteReference { reference_id } => {
                for graph in [base, latest] {
                    if let Some(reference) = graph.references.get(reference_id) {
                        insert_symbol_paths(base, latest, reference.from_symbol_id, &mut paths);
                        insert_symbol_paths(base, latest, reference.to_symbol_id, &mut paths);
                    }
                }
            }
        }
    }
    paths
}

fn insert_symbol_paths(
    base: &SemanticGraph,
    latest: &SemanticGraph,
    symbol_id: Uuid,
    paths: &mut BTreeSet<String>,
) {
    for graph in [base, latest] {
        if let Some(path) = file_path_for_symbol(graph, symbol_id) {
            paths.insert(path);
        }
    }
}

fn file_path_for_symbol(graph: &SemanticGraph, symbol_id: Uuid) -> Option<String> {
    let file = nearest_file_symbol(graph, graph.symbols.get(&symbol_id)?)?;
    Some(metadata_string(&file.metadata, "path").unwrap_or_else(|| file.name.clone()))
}

fn nearest_file_symbol<'a>(
    graph: &'a SemanticGraph,
    symbol: &'a SymbolNode,
) -> Option<&'a SymbolNode> {
    let mut current = symbol;
    loop {
        if current.kind == "file" {
            return Some(current);
        }
        current = graph.symbols.get(&current.parent_id?)?;
    }
}

pub(super) async fn run(
    command: SessionCommand,
    config: &Config,
    root: &Path,
    explicit_database_url: Option<&str>,
) -> Result<()> {
    match command {
        SessionCommand::Start(args) => {
            start_session(
                config,
                root,
                explicit_database_url,
                args.path,
                args.repo,
                args.branch,
                args.reset,
                args.no_validate,
            )
            .await
        }
        SessionCommand::Check(args) => {
            let path = args.path.unwrap_or_else(|| root.to_path_buf());
            let report = check(
                crate::plugins::language_registry(config),
                &path,
                &config.format,
            )
            .await?;
            report.print(&path);
            if !report.is_clean() {
                bail!(
                    "round-trip is not diff-clean: {} reformatted, {} dropped, {} added",
                    report.reformatted.len(),
                    report.dropped.len(),
                    report.added.len()
                );
            }
            println!(
                "round-trip clean: {} files reproduce byte-for-byte",
                report.clean.len()
            );
            Ok(())
        }
        SessionCommand::Review(args) => {
            let manifest = read_session_manifest(root).await?;
            let database_url =
                resolve_session_database_url(root, manifest.as_ref(), explicit_database_url);
            let (repository_name, branch_name) =
                resolve_session_ref(root, manifest.as_ref(), args.repo, args.branch)?;
            let storage =
                Storage::connect(&database_url, crate::plugins::language_registry(config)).await?;
            storage.migrate().await?;
            explorer::serve(
                storage,
                root.to_path_buf(),
                repository_name,
                branch_name,
                explorer::config_label(root),
                explorer::database_label(&database_url),
                args.addr,
                args.open,
            )
            .await
        }
        SessionCommand::Land(args) => {
            let manifest = read_session_manifest(root).await?;
            let database_url =
                resolve_session_database_url(root, manifest.as_ref(), explicit_database_url);
            let (repository_name, branch_name) =
                resolve_session_ref(root, manifest.as_ref(), args.repo, args.branch)?;
            // Writing in place into the working tree is gated so bonhomme never clobbers a user's
            // files unless they opted in; an explicit --out directory is always allowed.
            let writing_in_place = args.out.is_none();
            let dest = args.out.unwrap_or_else(|| root.to_path_buf());
            if writing_in_place && !config.git.write_back && !args.force {
                bail!(
                    "refusing to write into the working tree: enable `git.write_back = true` in \
                     bonhomme.toml, pass --out <dir>, or --force"
                );
            }

            let storage =
                Storage::connect(&database_url, crate::plugins::language_registry(config)).await?;
            storage.migrate().await?;
            let materialized = storage
                .materialize_branch(&repository_name, &branch_name)
                .await?;
            let base_position = manifest
                .as_ref()
                .filter(|manifest| {
                    manifest.repository == repository_name && manifest.branch == branch_name
                })
                .map(|manifest| manifest.base_position)
                .unwrap_or(0);
            let base = storage
                .materialize_branch_at_position(materialized.branch.id, base_position)
                .await?;
            let start = usize::try_from(base_position)
                .context("session base position does not fit in memory on this platform")?;
            let changed_operations = materialized.operations.get(start..).with_context(|| {
                format!(
                    "session base position {base_position} is beyond branch operation count {}",
                    materialized.operations.len()
                )
            })?;
            let changed_paths =
                changed_file_paths_since(&base.graph, &materialized.graph, changed_operations);
            // Apply the configured formatter to what we write, so a touched file lands matching the
            // repo's style instead of bonhomme's canonical render.
            let files = crate::format::format_files(materialized.files, &config.format).await;
            let stats = land_changed_files(&files, &dest, &changed_paths).await?;
            println!(
                "landed {} changed files into {} ({} removed, {} touched paths since op {})",
                stats.written,
                dest.display(),
                stats.deleted,
                changed_paths.len(),
                base_position
            );
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn blob_tree_round_trips_byte_identical() {
        // A text file and a binary file: neither is claimed by a structural plugin, so the blob
        // handler must reproduce both verbatim — the foundation write-back stands on.
        let dir = std::env::temp_dir().join(format!("bonhomme-rt-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("docs")).unwrap();
        std::fs::write(dir.join("notes.txt"), "hello\nworld\n").unwrap();
        std::fs::write(dir.join("docs").join("blob.bin"), [0u8, 1, 2, 3, 0, 255]).unwrap();

        let report = check(
            crate::plugins::language_registry(&Config::default()),
            &dir,
            &BTreeMap::new(),
        )
        .await
        .unwrap();

        assert!(
            report.is_clean(),
            "expected clean blob round-trip: {report:?}"
        );
        assert!(
            report.dropped.is_empty() && report.added.is_empty(),
            "{report:?}"
        );
        assert!(
            report.clean.iter().any(|p| p.ends_with("notes.txt")),
            "{report:?}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn land_removes_orphaned_files() {
        // A second land that no longer produces a file (a delete, or the old half of a move) must
        // remove the stale path so the Git diff is clean — and prune the now-empty directory.
        let dir = std::env::temp_dir().join(format!("bonhomme-land-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = |path: &str, content: &str| RenderedFile {
            path: path.into(),
            content: content.into(),
        };

        let first = land_tree(&[file("a.txt", "A"), file("sub/b.txt", "B")], &dir)
            .await
            .unwrap();
        assert_eq!(first.written, 2);
        assert_eq!(first.deleted, 0);
        assert!(dir.join("sub/b.txt").exists());

        // Re-land without sub/b.txt: it is now an orphan and must be removed, sub/ pruned.
        let second = land_tree(&[file("a.txt", "A2")], &dir).await.unwrap();
        assert_eq!(second.deleted, 1);
        assert!(!dir.join("sub/b.txt").exists(), "orphan should be deleted");
        assert!(!dir.join("sub").exists(), "empty dir should be pruned");
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "A2");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn land_changed_files_writes_only_touched_paths() {
        let dir =
            std::env::temp_dir().join(format!("bonhomme-land-changed-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "old\n").unwrap();
        std::fs::write(dir.join("b.txt"), "keep\n").unwrap();

        let files = vec![
            RenderedFile {
                path: "a.txt".into(),
                content: "new\n".into(),
            },
            RenderedFile {
                path: "b.txt".into(),
                content: "rendered but untouched\n".into(),
            },
        ];
        let changed_paths = BTreeSet::from(["a.txt".to_string()]);

        let stats = land_changed_files(&files, &dir, &changed_paths)
            .await
            .unwrap();

        assert_eq!(stats.written, 1);
        assert_eq!(stats.deleted, 0);
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "new\n");
        assert_eq!(
            std::fs::read_to_string(dir.join("b.txt")).unwrap(),
            "keep\n"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn changed_paths_map_child_symbol_updates_to_their_file() {
        let file_id = Uuid::new_v4();
        let child_id = Uuid::new_v4();
        let base_ops = vec![
            Operation::CreateSymbol {
                symbol_id: file_id,
                parent_id: None,
                kind: "file".into(),
                name: "app.ts".into(),
                body: None,
                metadata: json!({ "path": "src/app.ts" }),
            },
            Operation::CreateSymbol {
                symbol_id: child_id,
                parent_id: Some(file_id),
                kind: "function".into(),
                name: "run".into(),
                body: Some("old".into()),
                metadata: json!({}),
            },
        ];
        let update = Operation::UpdateSymbol {
            symbol_id: child_id,
            name: None,
            body: Some("new".into()),
            metadata: None,
        };
        let base_records = records(&base_ops);
        let mut latest_ops = base_ops;
        latest_ops.push(update.clone());
        let latest_records = records(&latest_ops);
        let base = bonhomme_core::materialize(&base_records).unwrap();
        let latest = bonhomme_core::materialize(&latest_records).unwrap();
        let changed_records = records(&[update]);

        let paths = changed_file_paths_since(&base, &latest, &changed_records);

        assert_eq!(paths, BTreeSet::from(["src/app.ts".to_string()]));
    }

    fn records(operations: &[Operation]) -> Vec<OperationRecord> {
        operations
            .iter()
            .enumerate()
            .map(|(index, operation)| OperationRecord {
                id: Uuid::new_v4(),
                repository_id: Uuid::nil(),
                branch_id: Uuid::nil(),
                changeset_id: Uuid::nil(),
                position: index as i64 + 1,
                operation: operation.clone(),
                created_at: chrono::DateTime::<chrono::Utc>::UNIX_EPOCH,
            })
            .collect()
    }
}
