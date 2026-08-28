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
    Handler, LanguagePlugin, Operation, PluginCapabilities, RenderedFile, SemanticGraph, Slice,
    ValidateFuture, ValidationStrength,
};
use std::path::Path;
use uuid::Uuid;

pub use import::import_php_files;
pub use recover::recover_php_operations;
pub use render::{render_files, render_slice};
pub use source::read_php_tree;
pub use validate::validate_php_files;

#[derive(Clone, Debug, Default)]
pub struct PhpPlugin {
    php: Option<String>,
}

impl PhpPlugin {
    pub fn with_php(php: impl Into<Option<String>>) -> Self {
        Self { php: php.into() }
    }
}

impl Handler for PhpPlugin {
    fn name(&self) -> &str {
        "php"
    }

    fn claims(&self, file: &RenderedFile) -> bool {
        source::is_php_source(file)
    }
}

impl LanguagePlugin for PhpPlugin {
    fn capabilities(&self) -> PluginCapabilities {
        PluginCapabilities::semantic(ValidationStrength::Syntax).with_limitations(&[
            "dynamic dispatch, namespace aliases, and Composer project semantics are not fully modeled",
            "validation is syntax-only",
        ])
    }

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
        import_php_files(files)
    }

    fn diff(&self, original: &[RenderedFile], modified: &[RenderedFile]) -> Result<Vec<Operation>> {
        let operations = import_php_files(original)?;
        let mut base = SemanticGraph::default();
        for operation in operations {
            base.apply_operation(Uuid::new_v4(), &operation)?;
        }
        recover_php_operations(&base, &[], modified)
    }

    fn recover_operations(
        &self,
        base: &SemanticGraph,
        scope: &[Uuid],
        edited: &[RenderedFile],
    ) -> Result<Vec<Operation>> {
        recover_php_operations(base, scope, edited)
    }

    fn read_source_tree(&self, root: &Path) -> Result<Vec<RenderedFile>> {
        read_php_tree(root)
    }

    fn validate<'a>(&'a self, files: &'a [RenderedFile]) -> ValidateFuture<'a> {
        Box::pin(validate::validate_php_files_with_php(
            files,
            self.php.as_deref(),
        ))
    }
}
