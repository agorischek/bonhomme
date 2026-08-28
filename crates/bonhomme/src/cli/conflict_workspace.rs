use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use bonhomme_core::{RenderedFile, decode_binary, encode_binary, safe_relative_path};
use bonhomme_engine::{ConflictRecord, ConflictViews};
use serde::{Deserialize, Serialize};
use tokio::fs;
use uuid::Uuid;

const MANIFEST_NAME: &str = ".bonhomme-conflict.json";
const ACTIVE_CONFLICT_NAME: &str = "active-conflict.json";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ConflictWorkspaceManifest {
    pub version: u32,
    pub id: Uuid,
    pub repository_id: Uuid,
    pub kind: String,
    pub state: String,
    pub source_branch: String,
    pub target_branch: String,
    pub base_position: i64,
    pub target_position: i64,
}

#[derive(Clone, Debug)]
pub(crate) struct CheckedOutConflict {
    pub manifest: ConflictWorkspaceManifest,
    pub resolved_files: Vec<RenderedFile>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ActiveConflictManifest {
    version: u32,
    id: Uuid,
    directory: String,
}

pub(crate) async fn checkout_conflict(
    root: &Path,
    views: &ConflictViews,
    out: Option<&Path>,
) -> Result<PathBuf> {
    let directory = checkout_directory(root, views.conflict.id, out);
    reject_symlink_components(root, &directory)?;
    ensure_checkout_directory_available(&directory).await?;
    fs::create_dir_all(&directory).await?;

    write_projection(&directory, "base", &views.base_files).await?;
    write_projection(&directory, "current", &views.current_files).await?;
    write_projection(&directory, "proposed", &views.proposed_files).await?;
    write_projection(&directory, "resolved", &views.resolved_files).await?;

    let manifest = manifest_from_conflict(&views.conflict);
    fs::write(
        directory.join(MANIFEST_NAME),
        serde_json::to_vec_pretty(&manifest)?,
    )
    .await?;
    fs::write(
        directory.join("CONFLICT.md"),
        conflict_markdown(&views.conflict)?,
    )
    .await?;
    write_active_conflict(root, &directory, views.conflict.id).await?;
    Ok(directory)
}

pub(crate) async fn read_checked_out_conflict(
    root: &Path,
    directory: Option<&Path>,
) -> Result<CheckedOutConflict> {
    let directory = match directory {
        Some(directory) => resolve_directory(root, directory),
        None => read_active_conflict_directory(root).await?,
    };
    let workspace = workspace_directory(&directory);
    reject_symlink_components(root, &workspace)?;
    let manifest_path = workspace.join(MANIFEST_NAME);
    reject_symlink_path(&manifest_path, "conflict manifest")?;
    let raw = fs::read_to_string(&manifest_path).await.with_context(|| {
        format!(
            "failed to read conflict manifest {}",
            manifest_path.display()
        )
    })?;
    let manifest: ConflictWorkspaceManifest = serde_json::from_str(&raw).with_context(|| {
        format!(
            "failed to parse conflict manifest {}",
            manifest_path.display()
        )
    })?;
    if manifest.version != 1 {
        bail!(
            "unsupported conflict workspace manifest version {} in {}",
            manifest.version,
            manifest_path.display()
        );
    }

    let resolved_directory = workspace.join("resolved");
    let resolved_files = read_tree_files(&resolved_directory).await?;
    Ok(CheckedOutConflict {
        manifest,
        resolved_files,
    })
}

pub(crate) fn display_directory(root: &Path, directory: &Path) -> String {
    directory
        .strip_prefix(root)
        .unwrap_or(directory)
        .display()
        .to_string()
}

fn manifest_from_conflict(conflict: &ConflictRecord) -> ConflictWorkspaceManifest {
    ConflictWorkspaceManifest {
        version: 1,
        id: conflict.id,
        repository_id: conflict.repository_id,
        kind: conflict.kind.to_string(),
        state: conflict.state.to_string(),
        source_branch: conflict.source_branch_name.clone(),
        target_branch: conflict.target_branch_name.clone(),
        base_position: conflict.base_position,
        target_position: conflict.target_position,
    }
}

fn checkout_directory(root: &Path, conflict_id: Uuid, out: Option<&Path>) -> PathBuf {
    if let Some(out) = out {
        return resolve_directory(root, out);
    }
    root.join(".bonhomme")
        .join("conflicts")
        .join(conflict_id.to_string())
}

fn resolve_directory(root: &Path, directory: &Path) -> PathBuf {
    if directory.is_absolute() {
        directory.to_path_buf()
    } else {
        root.join(directory)
    }
}

fn workspace_directory(directory: &Path) -> PathBuf {
    if directory.file_name().is_some_and(|name| name == "resolved") {
        directory.parent().unwrap_or(directory).to_path_buf()
    } else {
        directory.to_path_buf()
    }
}

async fn ensure_checkout_directory_available(directory: &Path) -> Result<()> {
    match fs::read_dir(directory).await {
        Ok(mut entries) => {
            if entries.next_entry().await?.is_some() {
                bail!(
                    "conflict checkout directory {} already exists and is not empty",
                    directory.display()
                );
            }
            Ok(())
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| {
            format!(
                "failed to inspect conflict checkout directory {}",
                directory.display()
            )
        }),
    }
}

async fn write_projection(directory: &Path, name: &str, files: &[RenderedFile]) -> Result<()> {
    let projection = directory.join(name);
    fs::create_dir_all(&projection).await?;
    for file in files {
        write_rendered_file(&projection, file).await?;
    }
    Ok(())
}

async fn write_rendered_file(directory: &Path, file: &RenderedFile) -> Result<()> {
    let relative = validate_workspace_path(&file.path)?;
    let path = directory.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await?;
    }
    match decode_binary(&file.content) {
        Some(bytes) => fs::write(&path, bytes).await?,
        None => fs::write(&path, &file.content).await?,
    }
    Ok(())
}

async fn read_tree_files(directory: &Path) -> Result<Vec<RenderedFile>> {
    reject_symlink_path(directory, "resolved conflict directory")?;
    let mut paths = Vec::new();
    collect_workspace_files(directory, directory, &mut paths)?;
    paths.sort();
    let mut files = Vec::with_capacity(paths.len());
    for rendered_path in paths {
        let relative = validate_workspace_path(&rendered_path)?;
        let path = directory.join(relative);
        let bytes = fs::read(&path)
            .await
            .with_context(|| format!("failed to read resolved file {}", path.display()))?;
        let content = match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(error) => encode_binary(&error.into_bytes()),
        };
        files.push(RenderedFile {
            path: rendered_path,
            content,
        });
    }
    Ok(files)
}

fn reject_symlink_path(path: &Path, label: &str) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("failed to inspect {label} {}", path.display()))?;
    if metadata.file_type().is_symlink() {
        bail!("{label} {} must not be a symlink", path.display());
    }
    Ok(())
}

fn reject_symlink_components(root: &Path, target: &Path) -> Result<()> {
    let (mut current, components): (PathBuf, Vec<_>) = match target.strip_prefix(root) {
        Ok(relative) => (root.to_path_buf(), relative.components().collect()),
        Err(_) => (PathBuf::new(), target.components().collect()),
    };
    for component in components {
        current.push(component.as_os_str());
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                bail!(
                    "conflict workspace path {} must not traverse symlinks",
                    current.display()
                );
            }
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("failed to inspect conflict path {}", current.display())
                });
            }
        }
    }
    Ok(())
}

fn collect_workspace_files(base: &Path, directory: &Path, files: &mut Vec<String>) -> Result<()> {
    for entry in std::fs::read_dir(directory)
        .with_context(|| format!("failed to read conflict directory {}", directory.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path)
            .with_context(|| format!("failed to inspect conflict path {}", path.display()))?;
        if metadata.file_type().is_symlink() {
            bail!(
                "conflict workspace path {} must not be a symlink",
                path.display()
            );
        }
        if metadata.is_dir() {
            collect_workspace_files(base, &path, files)?;
        } else if metadata.is_file() {
            files.push(relative_workspace_path(base, &path)?);
        }
    }
    Ok(())
}

fn relative_workspace_path(base: &Path, path: &Path) -> Result<String> {
    let relative = path
        .strip_prefix(base)
        .with_context(|| format!("{} is outside {}", path.display(), base.display()))?;
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            std::path::Component::Normal(part) => {
                parts.push(part.to_string_lossy().to_string());
            }
            _ => bail!(
                "conflict workspace path {} is not normalized",
                path.display()
            ),
        }
    }
    Ok(parts.join("/"))
}

fn validate_workspace_path(path: &str) -> Result<PathBuf> {
    let relative = safe_relative_path(path)?;
    if relative == Path::new(MANIFEST_NAME) {
        bail!("conflict file path {path} conflicts with Bonhomme conflict manifest");
    }
    Ok(relative)
}

async fn write_active_conflict(root: &Path, directory: &Path, conflict_id: Uuid) -> Result<()> {
    let active_path = active_conflict_path(root);
    if let Some(parent) = active_path.parent() {
        fs::create_dir_all(parent).await?;
    }
    let directory = directory
        .strip_prefix(root)
        .unwrap_or(directory)
        .display()
        .to_string();
    let active = ActiveConflictManifest {
        version: 1,
        id: conflict_id,
        directory,
    };
    fs::write(active_path, serde_json::to_vec_pretty(&active)?).await?;
    Ok(())
}

async fn read_active_conflict_directory(root: &Path) -> Result<PathBuf> {
    let active_path = active_conflict_path(root);
    let raw = fs::read_to_string(&active_path).await.with_context(|| {
        "no active conflict workspace found; run `bonhomme conflict checkout` first or pass --dir"
    })?;
    let active: ActiveConflictManifest = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse active conflict {}", active_path.display()))?;
    if active.version != 1 {
        bail!(
            "unsupported active conflict manifest version {} in {}",
            active.version,
            active_path.display()
        );
    }
    Ok(resolve_directory(root, Path::new(&active.directory)))
}

fn active_conflict_path(root: &Path) -> PathBuf {
    root.join(".bonhomme").join(ACTIVE_CONFLICT_NAME)
}

fn conflict_markdown(conflict: &ConflictRecord) -> Result<String> {
    let mut out = String::new();
    out.push_str(&format!("# Bonhomme conflict {}\n\n", conflict.id));
    out.push_str(&format!("- kind: `{}`\n", conflict.kind));
    out.push_str(&format!("- state: `{}`\n", conflict.state));
    out.push_str(&format!(
        "- source branch: `{}`\n",
        conflict.source_branch_name
    ));
    out.push_str(&format!(
        "- target branch: `{}`\n",
        conflict.target_branch_name
    ));
    out.push_str(&format!("- base position: `{}`\n", conflict.base_position));
    out.push_str(&format!(
        "- target position: `{}`\n\n",
        conflict.target_position
    ));
    out.push_str("## Conflicts\n\n");
    for item in &conflict.conflicts {
        out.push_str(&format!(
            "- `{}`: {}\n",
            item.reason,
            item.detail.replace('\n', " ")
        ));
    }
    if conflict.conflicts.is_empty() {
        out.push_str("- no structured merge conflicts were recorded\n");
    }
    out.push_str("\n## Proposed operations\n\n```json\n");
    out.push_str(&serde_json::to_string_pretty(
        &conflict.proposed_operations,
    )?);
    out.push_str("\n```\n\n");
    out.push_str("Edit only files under `resolved/`, then run `bonhomme conflict resolve`.\n");
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bonhomme_engine::{ConflictKind, ConflictState};
    use chrono::Utc;

    #[tokio::test]
    async fn checkout_and_read_conflict_workspace_allows_added_resolved_files() {
        let root = std::env::temp_dir().join(format!(
            "bonhomme-conflict-workspace-{}-{}",
            std::process::id(),
            Uuid::new_v4()
        ));
        let _ = fs::remove_dir_all(&root).await;
        fs::create_dir_all(&root).await.unwrap();

        let conflict = ConflictRecord {
            id: Uuid::new_v4(),
            repository_id: Uuid::new_v4(),
            kind: ConflictKind::SliceApply,
            state: ConflictState::Open,
            source_branch_id: Some(Uuid::new_v4()),
            target_branch_id: Some(Uuid::new_v4()),
            source_branch_name: "agent".to_string(),
            target_branch_name: "agent".to_string(),
            slice_id: Some(Uuid::new_v4()),
            source_changeset_id: None,
            base_position: 1,
            target_position: 2,
            conflicts: Vec::new(),
            proposed_operations: Vec::new(),
            resolution_changeset_id: None,
            acknowledged_drop: false,
            created_by: "agent".to_string(),
            resolved_by: None,
            created_at: Utc::now(),
            resolved_at: None,
        };
        let current_file = RenderedFile {
            path: "src/lib.ts".to_string(),
            content: "export const current = 1;\n".to_string(),
        };
        let views = ConflictViews {
            conflict: conflict.clone(),
            base_files: Vec::new(),
            current_files: vec![current_file.clone()],
            proposed_files: vec![RenderedFile {
                path: "src/lib.ts".to_string(),
                content: "export const proposed = 2;\n".to_string(),
            }],
            resolved_files: vec![current_file],
        };

        let directory = checkout_conflict(&root, &views, None).await.unwrap();
        fs::write(
            directory.join("resolved").join("src").join("extra.ts"),
            "export const extra = 3;\n",
        )
        .await
        .unwrap();

        let checked = read_checked_out_conflict(&root, Some(&directory))
            .await
            .unwrap();
        assert_eq!(checked.manifest.id, conflict.id);
        assert!(checked.resolved_files.iter().any(|file| {
            file.path == "src/extra.ts" && file.content == "export const extra = 3;\n"
        }));

        let _ = fs::remove_dir_all(&root).await;
    }

    fn test_views(conflict_id: Uuid) -> ConflictViews {
        ConflictViews {
            conflict: ConflictRecord {
                id: conflict_id,
                repository_id: Uuid::new_v4(),
                kind: ConflictKind::SliceApply,
                state: ConflictState::Open,
                source_branch_id: Some(Uuid::new_v4()),
                target_branch_id: Some(Uuid::new_v4()),
                source_branch_name: "agent".into(),
                target_branch_name: "agent".into(),
                slice_id: Some(Uuid::new_v4()),
                source_changeset_id: None,
                base_position: 1,
                target_position: 2,
                conflicts: Vec::new(),
                proposed_operations: Vec::new(),
                resolution_changeset_id: None,
                acknowledged_drop: false,
                created_by: "agent".into(),
                resolved_by: None,
                created_at: Utc::now(),
                resolved_at: None,
            },
            base_files: Vec::new(),
            current_files: Vec::new(),
            proposed_files: Vec::new(),
            resolved_files: Vec::new(),
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn conflict_workspace_rejects_symlinked_checkout_and_resolved_entries() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!(
            "bonhomme-conflict-symlink-{}-{}",
            std::process::id(),
            Uuid::new_v4()
        ));
        let outside = root.with_extension("outside");
        fs::create_dir_all(root.join(".bonhomme").join("conflicts"))
            .await
            .unwrap();
        fs::create_dir_all(&outside).await.unwrap();

        let precreated_id = Uuid::new_v4();
        let precreated = root
            .join(".bonhomme")
            .join("conflicts")
            .join(precreated_id.to_string());
        symlink(&outside, &precreated).unwrap();
        assert!(
            checkout_conflict(&root, &test_views(precreated_id), None)
                .await
                .is_err()
        );
        fs::remove_file(&precreated).await.unwrap();

        let conflict_id = Uuid::new_v4();
        let directory = checkout_conflict(&root, &test_views(conflict_id), None)
            .await
            .unwrap();
        let outside_file = outside.join("secret.txt");
        fs::write(&outside_file, "secret").await.unwrap();
        let file_link = directory.join("resolved").join("leak.txt");
        symlink(&outside_file, &file_link).unwrap();
        assert!(
            read_checked_out_conflict(&root, Some(&directory))
                .await
                .is_err()
        );
        fs::remove_file(&file_link).await.unwrap();

        let outside_directory = outside.join("directory");
        fs::create_dir_all(&outside_directory).await.unwrap();
        let directory_link = directory.join("resolved").join("leak-dir");
        symlink(&outside_directory, &directory_link).unwrap();
        assert!(
            read_checked_out_conflict(&root, Some(&directory))
                .await
                .is_err()
        );

        let _ = fs::remove_dir_all(&root).await;
        let _ = fs::remove_dir_all(&outside).await;
    }
}
