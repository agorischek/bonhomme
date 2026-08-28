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

pub use import::import_kotlin_files;
pub use recover::recover_kotlin_operations;
pub use render::{render_files, render_slice};
pub use source::read_kotlin_tree;
pub use validate::validate_kotlin_files;

#[derive(Clone, Debug, Default)]
pub struct KotlinPlugin {
    kotlinc: Option<String>,
}

impl KotlinPlugin {
    pub fn with_kotlinc(kotlinc: impl Into<Option<String>>) -> Self {
        Self {
            kotlinc: kotlinc.into(),
        }
    }
}

impl Handler for KotlinPlugin {
    fn name(&self) -> &str {
        "kotlin"
    }

    fn claims(&self, file: &RenderedFile) -> bool {
        source::is_kotlin_source(file)
    }
}

impl LanguagePlugin for KotlinPlugin {
    fn capabilities(&self) -> PluginCapabilities {
        PluginCapabilities::semantic(ValidationStrength::Compiler).with_limitations(&[
            "delegated properties, extension resolution, and Gradle project semantics are not fully modeled",
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
        import_kotlin_files(files)
    }

    fn diff(&self, original: &[RenderedFile], modified: &[RenderedFile]) -> Result<Vec<Operation>> {
        let operations = import_kotlin_files(original)?;
        let mut base = SemanticGraph::default();
        for operation in operations {
            base.apply_operation(Uuid::new_v4(), &operation)?;
        }
        recover_kotlin_operations(&base, &[], modified)
    }

    fn recover_operations(
        &self,
        base: &SemanticGraph,
        scope: &[Uuid],
        edited: &[RenderedFile],
    ) -> Result<Vec<Operation>> {
        recover_kotlin_operations(base, scope, edited)
    }

    fn read_source_tree(&self, root: &Path) -> Result<Vec<RenderedFile>> {
        read_kotlin_tree(root)
    }

    fn validate<'a>(&'a self, files: &'a [RenderedFile]) -> ValidateFuture<'a> {
        Box::pin(validate::validate_kotlin_files_with_kotlinc(
            files,
            self.kotlinc.as_deref(),
        ))
    }
}
