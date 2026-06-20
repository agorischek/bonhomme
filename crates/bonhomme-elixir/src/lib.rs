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

pub use import::import_elixir_files;
pub use recover::recover_elixir_operations;
pub use render::{render_files, render_slice};
pub use source::read_elixir_tree;
pub use validate::validate_elixir_files;

#[derive(Clone, Debug, Default)]
pub struct ElixirPlugin {
    compiler: Option<String>,
}

impl ElixirPlugin {
    pub fn with_compiler(compiler: impl Into<Option<String>>) -> Self {
        Self {
            compiler: compiler.into(),
        }
    }
}

impl Handler for ElixirPlugin {
    fn name(&self) -> &str {
        "elixir"
    }

    fn claims(&self, file: &RenderedFile) -> bool {
        source::is_elixir_source(file)
    }
}

impl LanguagePlugin for ElixirPlugin {
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
        import_elixir_files(files)
    }

    fn diff(&self, original: &[RenderedFile], modified: &[RenderedFile]) -> Result<Vec<Operation>> {
        let operations = import_elixir_files(original)?;
        let mut base = SemanticGraph::default();
        for operation in operations {
            base.apply_operation(Uuid::new_v4(), &operation)?;
        }
        recover_elixir_operations(&base, &[], modified)
    }

    fn recover_operations(
        &self,
        base: &SemanticGraph,
        scope: &[Uuid],
        edited: &[RenderedFile],
    ) -> Result<Vec<Operation>> {
        recover_elixir_operations(base, scope, edited)
    }

    fn read_source_tree(&self, root: &Path) -> Result<Vec<RenderedFile>> {
        read_elixir_tree(root)
    }

    fn validate<'a>(&'a self, files: &'a [RenderedFile]) -> ValidateFuture<'a> {
        Box::pin(validate::validate_elixir_files_with_compiler(
            files,
            self.compiler.as_deref(),
        ))
    }
}
