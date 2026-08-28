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

pub use import::import_swift_files;
pub use recover::recover_swift_operations;
pub use render::{render_files, render_slice};
pub use source::read_swift_tree;
pub use validate::validate_swift_files;

#[derive(Clone, Debug, Default)]
pub struct SwiftPlugin {
    swiftc: Option<String>,
}

impl SwiftPlugin {
    pub fn with_swiftc(swiftc: impl Into<Option<String>>) -> Self {
        Self {
            swiftc: swiftc.into(),
        }
    }
}

impl Handler for SwiftPlugin {
    fn name(&self) -> &str {
        "swift"
    }

    fn claims(&self, file: &RenderedFile) -> bool {
        source::is_swift_source(file)
    }
}

impl LanguagePlugin for SwiftPlugin {
    fn capabilities(&self) -> PluginCapabilities {
        PluginCapabilities::semantic(ValidationStrength::Syntax).with_limitations(&[
            "extensions, protocol conformance, macros, and SwiftPM semantics are not fully modeled",
            "validation parses source but does not type-check it",
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
        import_swift_files(files)
    }

    fn diff(&self, original: &[RenderedFile], modified: &[RenderedFile]) -> Result<Vec<Operation>> {
        let operations = import_swift_files(original)?;
        let mut base = SemanticGraph::default();
        for operation in operations {
            base.apply_operation(Uuid::new_v4(), &operation)?;
        }
        recover_swift_operations(&base, &[], modified)
    }

    fn recover_operations(
        &self,
        base: &SemanticGraph,
        scope: &[Uuid],
        edited: &[RenderedFile],
    ) -> Result<Vec<Operation>> {
        recover_swift_operations(base, scope, edited)
    }

    fn read_source_tree(&self, root: &Path) -> Result<Vec<RenderedFile>> {
        read_swift_tree(root)
    }

    fn validate<'a>(&'a self, files: &'a [RenderedFile]) -> ValidateFuture<'a> {
        Box::pin(validate::validate_swift_files_with_swiftc(
            files,
            self.swiftc.as_deref(),
        ))
    }
}
