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

pub use import::import_java_files;
pub use recover::recover_java_operations;
pub use render::{render_files, render_slice};
pub use source::read_java_tree;
pub use validate::validate_java_files;

#[derive(Clone, Debug, Default)]
pub struct JavaPlugin {
    javac: Option<String>,
}

impl JavaPlugin {
    pub fn with_javac(javac: impl Into<Option<String>>) -> Self {
        Self {
            javac: javac.into(),
        }
    }
}

impl Handler for JavaPlugin {
    fn name(&self) -> &str {
        "java"
    }

    fn claims(&self, file: &RenderedFile) -> bool {
        source::is_java_source(file)
    }
}

impl LanguagePlugin for JavaPlugin {
    fn capabilities(&self) -> PluginCapabilities {
        PluginCapabilities::semantic(ValidationStrength::Compiler).with_limitations(&[
            "modules, local classes, and build-tool project semantics are not fully modeled",
            "call edges are not overload-resolved",
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
        import_java_files(files)
    }

    fn diff(&self, original: &[RenderedFile], modified: &[RenderedFile]) -> Result<Vec<Operation>> {
        let operations = import_java_files(original)?;
        let mut base = SemanticGraph::default();
        for operation in operations {
            base.apply_operation(Uuid::new_v4(), &operation)?;
        }
        recover_java_operations(&base, &[], modified)
    }

    fn recover_operations(
        &self,
        base: &SemanticGraph,
        scope: &[Uuid],
        edited: &[RenderedFile],
    ) -> Result<Vec<Operation>> {
        recover_java_operations(base, scope, edited)
    }

    fn read_source_tree(&self, root: &Path) -> Result<Vec<RenderedFile>> {
        read_java_tree(root)
    }

    fn validate<'a>(&'a self, files: &'a [RenderedFile]) -> ValidateFuture<'a> {
        Box::pin(validate::validate_java_files_with_javac(
            files,
            self.javac.as_deref(),
        ))
    }
}
