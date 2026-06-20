use anyhow::Result;
use bonhomme_core::{RenderedFile, read_source_files};
use std::path::Path;

pub fn read_go_tree(root: &Path) -> Result<Vec<RenderedFile>> {
    Ok(read_source_files(root)?
        .into_iter()
        .filter(is_go_source)
        .collect())
}

pub fn is_go_source(file: &RenderedFile) -> bool {
    file.path.ends_with(".go") && !file.path.ends_with("_test.go")
}
