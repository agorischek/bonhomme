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

pub use import::import_python_files;
pub use recover::recover_python_operations;
pub use render::{render_files, render_slice};
pub use source::read_python_tree;
pub use validate::validate_python_files;

#[derive(Clone, Debug, Default)]
pub struct PythonPlugin {
    interpreter: Option<String>,
}

impl PythonPlugin {
    pub fn with_interpreter(interpreter: impl Into<Option<String>>) -> Self {
        Self {
            interpreter: interpreter.into(),
        }
    }
}

impl Handler for PythonPlugin {
    fn name(&self) -> &str {
        "python"
    }

    fn claims(&self, file: &RenderedFile) -> bool {
        source::is_python_source(file)
    }
}

impl LanguagePlugin for PythonPlugin {
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
        import_python_files(files)
    }

    fn diff(&self, original: &[RenderedFile], modified: &[RenderedFile]) -> Result<Vec<Operation>> {
        let operations = import_python_files(original)?;
        let mut base = SemanticGraph::default();
        for operation in operations {
            base.apply_operation(Uuid::new_v4(), &operation)?;
        }
        recover_python_operations(&base, &[], modified)
    }

    fn recover_operations(
        &self,
        base: &SemanticGraph,
        scope: &[Uuid],
        edited: &[RenderedFile],
    ) -> Result<Vec<Operation>> {
        recover_python_operations(base, scope, edited)
    }

    fn read_source_tree(&self, root: &Path) -> Result<Vec<RenderedFile>> {
        read_python_tree(root)
    }

    fn validate<'a>(&'a self, files: &'a [RenderedFile]) -> ValidateFuture<'a> {
        Box::pin(validate::validate_python_files_with_interpreter(
            files,
            self.interpreter.as_deref(),
        ))
    }
}
