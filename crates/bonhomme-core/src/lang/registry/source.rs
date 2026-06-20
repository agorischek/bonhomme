use anyhow::{Context, Result};
use ignore::WalkBuilder;
use std::path::Path;

use crate::lang::{MAX_INLINE_BINARY_BYTES, RenderedFile, encode_binary};

/// Read every file under `root` (one walk, all extensions), skipping conventional build/VCS
/// directories. Text files are read as UTF-8; binary files are wrapped in a base64 envelope (so they
/// round-trip through the blob handler) up to [`MAX_INLINE_BINARY_BYTES`], beyond which they are
/// skipped with a warning until content-addressed storage lands. No single language owns "read the
/// tree" in a polyglot repo, so the router owns it; partitioning into handlers happens at import
/// time. Exposed so a standalone handler can read just the files it claims
/// (`read_source_files(root)?.into_iter().filter(|f| self.claims(f))`).
pub fn read_source_files(root: &Path) -> Result<Vec<RenderedFile>> {
    let mut files = Vec::new();
    let mut skipped_large = 0usize;
    let base = if root.is_file() {
        root.parent().unwrap_or_else(|| Path::new("."))
    } else {
        root
    };

    if root.is_file() {
        collect_file(root, base, &mut files, &mut skipped_large)?;
    } else {
        let walker = WalkBuilder::new(root)
            .hidden(false)
            .parents(true)
            .ignore(true)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .require_git(false)
            .filter_entry(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_none_or(|name| !IGNORED_DIRECTORIES.contains(&name))
            })
            .build();
        for entry in walker {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() {
                collect_file(path, base, &mut files, &mut skipped_large)?;
            }
        }
    }

    files.sort_by(|a, b| a.path.cmp(&b.path));
    if skipped_large > 0 {
        eprintln!(
            "bonhomme: skipped {skipped_large} binary file(s) larger than {} bytes during import",
            MAX_INLINE_BINARY_BYTES
        );
    }
    Ok(files)
}

/// Last-resort skips for common generated trees when a project has no ignore file. Git ignore
/// rules are the primary filter; this list keeps local/import metadata and dependency outputs out
/// of fresh or non-Git directories too.
const IGNORED_DIRECTORIES: &[&str] = &[
    "node_modules",
    "dist",
    "target",
    ".git",
    ".bonhomme",
    ".hg",
    ".svn",
    "build",
    "out",
    ".next",
    ".nuxt",
    "vendor",
    ".venv",
    "venv",
    "__pycache__",
    ".mypy_cache",
    ".pytest_cache",
    ".cargo",
    ".idea",
    ".vscode",
    ".studios",
    ".turbo",
    ".cache",
    ".parcel-cache",
    ".svelte-kit",
    ".astro",
    "coverage",
    "tmp",
    "temp",
];

fn collect_file(
    path: &Path,
    base: &Path,
    files: &mut Vec<RenderedFile>,
    skipped_large: &mut usize,
) -> Result<()> {
    if !path.is_file() {
        return Ok(());
    }

    let relative_path = path
        .strip_prefix(base)
        .unwrap_or(path)
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");

    match String::from_utf8(
        std::fs::read(path).with_context(|| format!("reading source file {}", path.display()))?,
    ) {
        Ok(content) => files.push(RenderedFile {
            path: relative_path,
            content,
        }),
        Err(error) => {
            // Not valid UTF-8: store as a base64 binary envelope if small enough to inline.
            let bytes = error.into_bytes();
            if bytes.len() <= MAX_INLINE_BINARY_BYTES {
                files.push(RenderedFile {
                    path: relative_path,
                    content: encode_binary(&bytes),
                });
            } else {
                *skipped_large += 1;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_bonhomme_session_state() {
        let dir = std::env::temp_dir().join(format!("bonhomme-source-skip-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".bonhomme")).unwrap();
        std::fs::write(dir.join("src.txt"), "tracked\n").unwrap();
        std::fs::write(dir.join(".bonhomme").join("session.json"), "{}\n").unwrap();

        let files = read_source_files(&dir).unwrap();
        let paths = files.into_iter().map(|file| file.path).collect::<Vec<_>>();

        assert_eq!(paths, vec!["src.txt"]);
        std::fs::remove_dir_all(&dir).ok();
    }
}
