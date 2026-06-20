mod import;
mod model;
mod recover;
mod render;
mod source;
mod toolchain;
mod validate;

#[cfg(test)]
mod tests;

use anyhow::Result;
use bonhomme_core::{
    Handler, LanguagePlugin, Operation, RenderedFile, SemanticGraph, Slice, ValidateFuture,
};
use std::path::Path;
use uuid::Uuid;

pub use import::import_go_files;
pub use recover::recover_go_operations;
pub use render::{render_files, render_slice};
pub use source::read_go_tree;
pub use validate::validate_go_files;

#[derive(Clone, Copy, Debug, Default)]
pub struct GoPlugin;

impl Handler for GoPlugin {
    fn name(&self) -> &str {
        "go"
    }

    fn claims(&self, file: &RenderedFile) -> bool {
        source::is_go_source(file)
    }
}

impl LanguagePlugin for GoPlugin {
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
        import_go_files(files)
    }

    fn diff(&self, original: &[RenderedFile], modified: &[RenderedFile]) -> Result<Vec<Operation>> {
        let operations = import_go_files(original)?;
        let mut base = SemanticGraph::default();
        for operation in operations {
            base.apply_operation(Uuid::new_v4(), &operation)?;
        }
        recover_go_operations(&base, &[], modified)
    }

    fn recover_operations(
        &self,
        base: &SemanticGraph,
        scope: &[Uuid],
        edited: &[RenderedFile],
    ) -> Result<Vec<Operation>> {
        recover_go_operations(base, scope, edited)
    }

    fn read_source_tree(&self, root: &Path) -> Result<Vec<RenderedFile>> {
        read_go_tree(root)
    }

    fn validate<'a>(&'a self, files: &'a [RenderedFile]) -> ValidateFuture<'a> {
        Box::pin(validate_go_files(files))
    }
}
