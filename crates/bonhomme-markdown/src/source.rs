use anyhow::Result;
use bonhomme_core::RenderedFile;
use std::path::Path;

pub fn read_markdown_tree(root: &Path) -> Result<Vec<RenderedFile>> {
    Ok(bonhomme_core::read_source_files(root)?
        .into_iter()
        .filter(is_markdown_source)
        .collect())
}

pub(crate) fn is_markdown_source(file: &RenderedFile) -> bool {
    let path = file.path.to_ascii_lowercase();
    path.ends_with(".md") || path.ends_with(".markdown") || path.ends_with(".mdown")
}
