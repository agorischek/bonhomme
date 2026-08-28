use anyhow::Result;
use bonhomme_core::{RenderedFile, read_source_files};
use std::path::Path;

pub fn read_java_tree(root: &Path) -> Result<Vec<RenderedFile>> {
    Ok(read_source_files(root)?
        .into_iter()
        .filter(is_java_source)
        .collect())
}

pub fn is_java_source(file: &RenderedFile) -> bool {
    file.path.ends_with(".java")
}
