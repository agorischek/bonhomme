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

pub use import::import_ruby_files;
pub use recover::recover_ruby_operations;
pub use render::{render_files, render_slice};
pub use source::read_ruby_tree;
pub use validate::validate_ruby_files;

#[derive(Clone, Debug, Default)]
pub struct RubyPlugin {
    ruby: Option<String>,
}

impl RubyPlugin {
    pub fn with_ruby(ruby: impl Into<Option<String>>) -> Self {
        Self { ruby: ruby.into() }
    }
}

impl Handler for RubyPlugin {
    fn name(&self) -> &str {
        "ruby"
    }

    fn claims(&self, file: &RenderedFile) -> bool {
        source::is_ruby_source(file)
    }
}

impl LanguagePlugin for RubyPlugin {
    fn capabilities(&self) -> PluginCapabilities {
        PluginCapabilities::semantic(ValidationStrength::Syntax).with_limitations(&[
            "metaprogramming, mixins, refinements, and dynamic dispatch are not fully modeled",
            "validation is syntax-only and not Bundler-aware",
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
        import_ruby_files(files)
    }

    fn diff(&self, original: &[RenderedFile], modified: &[RenderedFile]) -> Result<Vec<Operation>> {
        let operations = import_ruby_files(original)?;
        let mut base = SemanticGraph::default();
        for operation in operations {
            base.apply_operation(Uuid::new_v4(), &operation)?;
        }
        recover_ruby_operations(&base, &[], modified)
    }

    fn recover_operations(
        &self,
        base: &SemanticGraph,
        scope: &[Uuid],
        edited: &[RenderedFile],
    ) -> Result<Vec<Operation>> {
        recover_ruby_operations(base, scope, edited)
    }

    fn read_source_tree(&self, root: &Path) -> Result<Vec<RenderedFile>> {
        read_ruby_tree(root)
    }

    fn validate<'a>(&'a self, files: &'a [RenderedFile]) -> ValidateFuture<'a> {
        Box::pin(validate::validate_ruby_files_with_ruby(
            files,
            self.ruby.as_deref(),
        ))
    }
}
