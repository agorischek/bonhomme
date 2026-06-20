mod ids;
mod import;
mod model;
mod recover;
mod render;
mod source;
mod validate;

#[cfg(test)]
mod tests;

use anyhow::Result;
use bonhomme_core::{
    Handler, LanguagePlugin, Operation, RenderedFile, SemanticGraph, Slice, ValidateFuture,
};
use std::path::Path;
use uuid::Uuid;

pub use import::import_rust_files;
pub use recover::recover_rust_operations;
pub use render::{render_files, render_slice};
pub use source::read_rust_tree;
pub use validate::validate_rust_files;

#[derive(Clone, Copy, Debug, Default)]
pub struct RustPlugin;

impl Handler for RustPlugin {
    fn name(&self) -> &str {
        "rust"
    }

    fn claims(&self, file: &RenderedFile) -> bool {
        source::is_rust_source(file)
    }
}

impl LanguagePlugin for RustPlugin {
    fn render(&self, graph: &SemanticGraph) -> Vec<RenderedFile> {
        render_files(graph)
    }

    fn render_slice(
        &self,
        graph: &SemanticGraph,
        base_revision: String,
        root_symbols: Vec<Uuid>,
    ) -> Slice {
        render_slice(graph, base_revision, root_symbols)
    }

    fn import(&self, files: &[RenderedFile]) -> Result<Vec<Operation>> {
        import_rust_files(files)
    }

    fn diff(&self, original: &[RenderedFile], modified: &[RenderedFile]) -> Result<Vec<Operation>> {
        let operations = import_rust_files(original)?;
        let mut base = SemanticGraph::default();
        for operation in operations {
            base.apply_operation(Uuid::new_v4(), &operation)?;
        }
        recover_rust_operations(&base, &[], modified)
    }

    fn recover_operations(
        &self,
        base: &SemanticGraph,
        scope: &[Uuid],
        edited: &[RenderedFile],
    ) -> Result<Vec<Operation>> {
        recover_rust_operations(base, scope, edited)
    }

    fn read_source_tree(&self, root: &Path) -> Result<Vec<RenderedFile>> {
        read_rust_tree(root)
    }

    fn validate<'a>(&'a self, files: &'a [RenderedFile]) -> ValidateFuture<'a> {
        Box::pin(validate_rust_files(files))
    }
}
