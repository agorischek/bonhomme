use anyhow::Result;
use bonhomme_core::RenderedFile;
use std::{fs as std_fs, path::Path};

const SUPPORTED_EXTENSIONS: &[&str] = &["ts", "tsx", "js", "jsx"];

pub fn read_typescript_tree(root: &Path) -> Result<Vec<RenderedFile>> {
    let mut files = Vec::new();
    let base = if root.is_file() {
        root.parent().unwrap_or_else(|| Path::new("."))
    } else {
        root
    };
    collect_typescript_files(root, base, &mut files)?;
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

pub fn is_typescript_source(file: &RenderedFile) -> bool {
    is_supported_source_path(Path::new(&file.path))
}

fn collect_typescript_files(path: &Path, base: &Path, files: &mut Vec<RenderedFile>) -> Result<()> {
    if path.is_dir() {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            return Ok(());
        };
        if matches!(name, "node_modules" | "dist" | "target" | ".git") {
            return Ok(());
        }
        for entry in std_fs::read_dir(path)? {
            collect_typescript_files(&entry?.path(), base, files)?;
        }
        return Ok(());
    }

    if !is_supported_source_path(path) {
        return Ok(());
    }

    let relative_path = path
        .strip_prefix(base)
        .unwrap_or(path)
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    files.push(RenderedFile {
        path: relative_path,
        content: std_fs::read_to_string(path)?,
    });
    Ok(())
}

fn is_supported_source_path(path: &Path) -> bool {
    if path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(".d.ts"))
    {
        return false;
    }

    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| SUPPORTED_EXTENSIONS.contains(&extension))
}
