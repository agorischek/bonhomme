use anyhow::Result;

use crate::lang::{MAX_INLINE_BINARY_BYTES, RenderedFile, encode_binary};

/// Read every file under `root` (one walk, all extensions), skipping conventional build/VCS
/// directories. Text files are read as UTF-8; binary files are wrapped in a base64 envelope (so they
/// round-trip through the blob handler) up to [`MAX_INLINE_BINARY_BYTES`], beyond which they are
/// skipped with a warning until content-addressed storage lands. No single language owns "read the
/// tree" in a polyglot repo, so the router owns it; partitioning into handlers happens at import
/// time. Exposed so a standalone handler can read just the files it claims
/// (`read_source_files(root)?.into_iter().filter(|f| self.claims(f))`).
pub fn read_source_files(root: &std::path::Path) -> Result<Vec<RenderedFile>> {
    let mut files = Vec::new();
    let mut skipped_large = 0usize;
    let base = if root.is_file() {
        root.parent().unwrap_or_else(|| std::path::Path::new("."))
    } else {
        root
    };
    collect_files(root, base, &mut files, &mut skipped_large)?;
    files.sort_by(|a, b| a.path.cmp(&b.path));
    if skipped_large > 0 {
        eprintln!(
            "bonhomme: skipped {skipped_large} binary file(s) larger than {} bytes during import",
            MAX_INLINE_BINARY_BYTES
        );
    }
    Ok(files)
}

const IGNORED_DIRECTORIES: &[&str] = &[
    "node_modules",
    "dist",
    "target",
    ".git",
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
];

fn collect_files(
    path: &std::path::Path,
    base: &std::path::Path,
    files: &mut Vec<RenderedFile>,
    skipped_large: &mut usize,
) -> Result<()> {
    if path.is_dir() {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            return Ok(());
        };
        if IGNORED_DIRECTORIES.contains(&name) {
            return Ok(());
        }
        let mut entries: Vec<_> = std::fs::read_dir(path)?
            .collect::<std::result::Result<Vec<_>, _>>()?
            .into_iter()
            .map(|entry| entry.path())
            .collect();
        entries.sort();
        for entry in entries {
            collect_files(&entry, base, files, skipped_large)?;
        }
        return Ok(());
    }

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

    match String::from_utf8(std::fs::read(path)?) {
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
