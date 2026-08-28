use std::{
    collections::BTreeSet,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use bonhomme_core::{RenderedFile, Slice, decode_binary, encode_binary, safe_relative_path};
use serde::{Deserialize, Serialize};
use tokio::fs;
use uuid::Uuid;

const MANIFEST_NAME: &str = ".bonhomme-slice.json";
const ACTIVE_SLICE_NAME: &str = "active-slice.json";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SliceWorkspaceManifest {
    pub version: u32,
    pub id: Uuid,
    pub repository: String,
    pub branch: String,
    pub base_revision: String,
    pub root_symbols: Vec<Uuid>,
    pub files: Vec<SliceWorkspaceFile>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SliceWorkspaceFile {
    pub path: String,
    pub encoding: SliceFileEncoding,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub(super) enum SliceFileEncoding {
    #[serde(rename = "utf-8")]
    Utf8,
    #[serde(rename = "base64")]
    Base64,
}

#[derive(Clone, Debug)]
pub(super) struct CheckedOutSlice {
    pub manifest: SliceWorkspaceManifest,
    pub files: Vec<RenderedFile>,
    pub directory: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ActiveSliceManifest {
    version: u32,
    id: Uuid,
    repository: String,
    branch: String,
    directory: String,
}

pub(super) async fn checkout_slice(
    root: &Path,
    repository: &str,
    branch: &str,
    slice: &Slice,
    out: Option<&Path>,
) -> Result<PathBuf> {
    let directory = checkout_directory(root, branch, slice.id, out);
    reject_symlink_components(root, &directory)?;
    ensure_checkout_directory_available(&directory).await?;
    fs::create_dir_all(&directory).await?;

    let manifest = SliceWorkspaceManifest {
        version: 1,
        id: slice.id,
        repository: repository.to_string(),
        branch: branch.to_string(),
        base_revision: slice.base_revision.clone(),
        root_symbols: slice.root_symbols.clone(),
        files: manifest_files(&slice.files)?,
    };

    for file in &slice.files {
        write_slice_file(&directory, file).await?;
    }
    fs::write(
        directory.join(MANIFEST_NAME),
        serde_json::to_vec_pretty(&manifest)?,
    )
    .await?;
    write_active_slice(root, &directory, &manifest).await?;
    Ok(directory)
}

pub(super) async fn read_checked_out_slice(
    root: &Path,
    directory: Option<&Path>,
) -> Result<CheckedOutSlice> {
    let directory = match directory {
        Some(directory) => resolve_directory(root, directory),
        None => read_active_slice_directory(root).await?,
    };
    reject_symlink_components(root, &directory)?;
    let manifest_path = directory.join(MANIFEST_NAME);
    reject_symlink_path(&manifest_path, "slice manifest")?;
    let raw = fs::read_to_string(&manifest_path)
        .await
        .with_context(|| format!("failed to read slice manifest {}", manifest_path.display()))?;
    let manifest: SliceWorkspaceManifest = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse slice manifest {}", manifest_path.display()))?;
    if manifest.version != 1 {
        bail!(
            "unsupported slice workspace manifest version {} in {}",
            manifest.version,
            manifest_path.display()
        );
    }

    let mut files = Vec::new();
    let mut seen = BTreeSet::new();
    for file in &manifest.files {
        if !seen.insert(file.path.clone()) {
            bail!("slice manifest lists duplicate file path {}", file.path);
        }
    }
    reject_unlisted_files(&directory, &seen)?;
    for file in &manifest.files {
        files.push(read_slice_file(&directory, file).await?);
    }
    Ok(CheckedOutSlice {
        manifest,
        files,
        directory,
    })
}

pub(super) fn display_directory(root: &Path, directory: &Path) -> String {
    directory
        .strip_prefix(root)
        .unwrap_or(directory)
        .display()
        .to_string()
}

fn checkout_directory(root: &Path, branch: &str, slice_id: Uuid, out: Option<&Path>) -> PathBuf {
    if let Some(out) = out {
        return resolve_directory(root, out);
    }
    root.join(".bonhomme")
        .join("slices")
        .join(slug(branch))
        .join(slice_id.to_string())
}

fn resolve_directory(root: &Path, directory: &Path) -> PathBuf {
    if directory.is_absolute() {
        directory.to_path_buf()
    } else {
        root.join(directory)
    }
}

async fn ensure_checkout_directory_available(directory: &Path) -> Result<()> {
    match fs::read_dir(directory).await {
        Ok(mut entries) => {
            if entries.next_entry().await?.is_some() {
                bail!(
                    "slice checkout directory {} already exists and is not empty",
                    directory.display()
                );
            }
            Ok(())
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| {
            format!(
                "failed to inspect slice checkout directory {}",
                directory.display()
            )
        }),
    }
}

fn manifest_files(files: &[RenderedFile]) -> Result<Vec<SliceWorkspaceFile>> {
    let mut seen = BTreeSet::new();
    let mut manifest_files = Vec::new();
    for file in files {
        validate_workspace_path(&file.path)?;
        if !seen.insert(file.path.clone()) {
            bail!("slice contains duplicate file path {}", file.path);
        }
        let encoding = if decode_binary(&file.content).is_some() {
            SliceFileEncoding::Base64
        } else {
            SliceFileEncoding::Utf8
        };
        manifest_files.push(SliceWorkspaceFile {
            path: file.path.clone(),
            encoding,
        });
    }
    Ok(manifest_files)
}

async fn write_slice_file(directory: &Path, file: &RenderedFile) -> Result<()> {
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

async fn read_slice_file(directory: &Path, file: &SliceWorkspaceFile) -> Result<RenderedFile> {
    let relative = validate_workspace_path(&file.path)?;
    let path = directory.join(relative);
    reject_symlink_path(&path, "checked-out slice file")?;
    let bytes = fs::read(&path)
        .await
        .with_context(|| format!("failed to read checked-out slice file {}", path.display()))?;
    let content = match file.encoding {
        SliceFileEncoding::Utf8 => String::from_utf8(bytes)
            .with_context(|| format!("checked-out slice file {} is not UTF-8", path.display()))?,
        SliceFileEncoding::Base64 => encode_binary(&bytes),
    };
    Ok(RenderedFile {
        path: file.path.clone(),
        content,
    })
}

fn reject_unlisted_files(directory: &Path, expected: &BTreeSet<String>) -> Result<()> {
    let mut actual = Vec::new();
    collect_workspace_files(directory, directory, &mut actual)?;
    for path in actual {
        if path == MANIFEST_NAME || expected.contains(&path) {
            continue;
        }
        bail!(
            "checked-out slice directory {} contains unlisted file {}; add-file slices are not supported yet",
            directory.display(),
            path
        );
    }
    Ok(())
}

fn collect_workspace_files(base: &Path, directory: &Path, files: &mut Vec<String>) -> Result<()> {
    for entry in std::fs::read_dir(directory)
        .with_context(|| format!("failed to read slice directory {}", directory.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path)
            .with_context(|| format!("failed to inspect slice path {}", path.display()))?;
        if metadata.file_type().is_symlink() {
            bail!(
                "slice workspace path {} must not be a symlink",
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
                    "slice workspace path {} must not traverse symlinks",
                    current.display()
                );
            }
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("failed to inspect slice path {}", current.display())
                });
            }
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
            _ => bail!("slice workspace path {} is not normalized", path.display()),
        }
    }
    Ok(parts.join("/"))
}

fn validate_workspace_path(path: &str) -> Result<PathBuf> {
    let relative = safe_relative_path(path)?;
    if relative == Path::new(MANIFEST_NAME) {
        bail!("slice file path {path} conflicts with Bonhomme slice manifest");
    }
    Ok(relative)
}

async fn write_active_slice(
    root: &Path,
    directory: &Path,
    manifest: &SliceWorkspaceManifest,
) -> Result<()> {
    let active_path = active_slice_path(root);
    if let Some(parent) = active_path.parent() {
        fs::create_dir_all(parent).await?;
    }
    let directory = directory
        .strip_prefix(root)
        .unwrap_or(directory)
        .display()
        .to_string();
    let active = ActiveSliceManifest {
        version: 1,
        id: manifest.id,
        repository: manifest.repository.clone(),
        branch: manifest.branch.clone(),
        directory,
    };
    fs::write(active_path, serde_json::to_vec_pretty(&active)?).await?;
    Ok(())
}

async fn read_active_slice_directory(root: &Path) -> Result<PathBuf> {
    let active_path = active_slice_path(root);
    let raw = fs::read_to_string(&active_path).await.with_context(|| {
        "no active slice workspace found; run `bonhomme slice create` first or pass --dir"
            .to_string()
    })?;
    let active: ActiveSliceManifest = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse active slice {}", active_path.display()))?;
    if active.version != 1 {
        bail!(
            "unsupported active slice manifest version {} in {}",
            active.version,
            active_path.display()
        );
    }
    Ok(resolve_directory(root, Path::new(&active.directory)))
}

fn active_slice_path(root: &Path) -> PathBuf {
    root.join(".bonhomme").join(ACTIVE_SLICE_NAME)
}

fn slug(value: &str) -> String {
    let slug = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if slug.is_empty() || slug == "." || slug == ".." {
        "branch".to_string()
    } else {
        slug
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_keeps_paths_out_of_default_checkout_directory() {
        assert_eq!(slug("feature/foo"), "feature_foo");
        assert_eq!(slug("../main"), ".._main");
        assert_eq!(slug(".."), "branch");
        assert_eq!(slug("."), "branch");
        assert_eq!(slug("agent-001"), "agent-001");
    }

    #[tokio::test]
    async fn checkout_and_read_slice_workspace_round_trips_text_and_binary() {
        let root = std::env::temp_dir().join(format!(
            "bonhomme-slice-workspace-{}-{}",
            std::process::id(),
            Uuid::new_v4()
        ));
        let _ = fs::remove_dir_all(&root).await;
        fs::create_dir_all(&root).await.unwrap();

        let slice = Slice {
            id: Uuid::new_v4(),
            base_revision: "agent-001@3".to_string(),
            root_symbols: vec![Uuid::new_v4()],
            files: vec![
                RenderedFile {
                    path: "src/lib.ts".to_string(),
                    content: "export const answer = 42;\n".to_string(),
                },
                RenderedFile {
                    path: "assets/logo.bin".to_string(),
                    content: encode_binary(&[0, 1, 2, 255]),
                },
            ],
        };

        let directory = checkout_slice(&root, "repo", "agent-001", &slice, None)
            .await
            .unwrap();
        let text = fs::read_to_string(directory.join("src/lib.ts"))
            .await
            .unwrap();
        assert_eq!(text, "export const answer = 42;\n");
        assert_eq!(
            fs::read(directory.join("assets/logo.bin")).await.unwrap(),
            vec![0, 1, 2, 255]
        );

        let checked_out = read_checked_out_slice(&root, None).await.unwrap();
        assert_eq!(checked_out.manifest.id, slice.id);
        assert_eq!(checked_out.manifest.repository, "repo");
        assert_eq!(checked_out.files, slice.files);

        let _ = fs::remove_dir_all(&root).await;
    }

    #[tokio::test]
    async fn missing_manifest_file_is_an_error() {
        let root = std::env::temp_dir().join(format!(
            "bonhomme-slice-workspace-missing-{}-{}",
            std::process::id(),
            Uuid::new_v4()
        ));
        let _ = fs::remove_dir_all(&root).await;
        fs::create_dir_all(&root).await.unwrap();
        let directory = root.join(".bonhomme").join("slices").join("agent");
        fs::create_dir_all(&directory).await.unwrap();
        fs::write(
            directory.join(MANIFEST_NAME),
            serde_json::to_vec_pretty(&SliceWorkspaceManifest {
                version: 1,
                id: Uuid::new_v4(),
                repository: "repo".to_string(),
                branch: "agent".to_string(),
                base_revision: "agent@1".to_string(),
                root_symbols: Vec::new(),
                files: vec![SliceWorkspaceFile {
                    path: "missing.ts".to_string(),
                    encoding: SliceFileEncoding::Utf8,
                }],
            })
            .unwrap(),
        )
        .await
        .unwrap();

        let error = read_checked_out_slice(&root, Some(&directory))
            .await
            .unwrap_err();
        assert!(format!("{error:#}").contains("missing.ts"));

        let _ = fs::remove_dir_all(&root).await;
    }

    #[tokio::test]
    async fn unlisted_workspace_file_is_an_error() {
        let root = std::env::temp_dir().join(format!(
            "bonhomme-slice-workspace-extra-{}-{}",
            std::process::id(),
            Uuid::new_v4()
        ));
        let _ = fs::remove_dir_all(&root).await;
        fs::create_dir_all(&root).await.unwrap();

        let slice = Slice {
            id: Uuid::new_v4(),
            base_revision: "agent@1".to_string(),
            root_symbols: Vec::new(),
            files: vec![RenderedFile {
                path: "src/lib.ts".to_string(),
                content: "export const x = 1;\n".to_string(),
            }],
        };
        let directory = checkout_slice(&root, "repo", "agent", &slice, None)
            .await
            .unwrap();
        fs::write(directory.join("src/extra.ts"), "export const y = 2;\n")
            .await
            .unwrap();

        let error = read_checked_out_slice(&root, Some(&directory))
            .await
            .unwrap_err();
        assert!(format!("{error:#}").contains("extra.ts"));

        let _ = fs::remove_dir_all(&root).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn slice_workspace_rejects_symlinked_checkout_and_listed_files() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!(
            "bonhomme-slice-symlink-{}-{}",
            std::process::id(),
            Uuid::new_v4()
        ));
        let outside = root.with_extension("outside");
        let _ = fs::remove_dir_all(&root).await;
        let _ = fs::remove_dir_all(&outside).await;
        fs::create_dir_all(root.join(".bonhomme").join("slices"))
            .await
            .unwrap();
        fs::create_dir_all(&outside).await.unwrap();

        let slice = Slice {
            id: Uuid::new_v4(),
            base_revision: "agent@1".to_string(),
            root_symbols: Vec::new(),
            files: vec![RenderedFile {
                path: "src/lib.ts".to_string(),
                content: "export const inside = true;\n".to_string(),
            }],
        };
        let checkout_parent = root.join(".bonhomme").join("slices").join("agent");
        symlink(&outside, &checkout_parent).unwrap();
        assert!(
            checkout_slice(&root, "repo", "agent", &slice, None)
                .await
                .is_err()
        );
        fs::remove_file(&checkout_parent).await.unwrap();

        let directory = checkout_slice(&root, "repo", "agent", &slice, None)
            .await
            .unwrap();
        let outside_file = outside.join("private.ts");
        fs::write(&outside_file, "export const outside = true;\n")
            .await
            .unwrap();
        let listed_file = directory.join("src/lib.ts");
        fs::remove_file(&listed_file).await.unwrap();
        symlink(&outside_file, &listed_file).unwrap();

        assert!(
            read_checked_out_slice(&root, Some(&directory))
                .await
                .is_err()
        );

        let _ = fs::remove_dir_all(&root).await;
        let _ = fs::remove_dir_all(&outside).await;
    }
}
