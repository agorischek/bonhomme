use anyhow::Result;
use bonhomme_core::{RenderedFile, read_source_files};
use std::path::Path;

pub fn read_kotlin_tree(root: &Path) -> Result<Vec<RenderedFile>> {
    Ok(read_source_files(root)?
        .into_iter()
        .filter(is_kotlin_source)
        .collect())
}

pub fn is_kotlin_source(file: &RenderedFile) -> bool {
    file.path.ends_with(".kt") || file.path.ends_with(".kts")
}
