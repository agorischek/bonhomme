mod diff;
mod import;
mod oxc_parse;
mod parse;
mod recover;
mod render;
mod scanner;
mod source;
mod validate;

#[cfg(test)]
mod tests;

use anyhow::Result;
use bonhomme_core::{
    LanguagePlugin, Operation, RenderedFile, SemanticGraph, Slice, ValidateFuture,
};
use std::path::Path;
use uuid::Uuid;

pub use diff::diff_slice;
pub use import::import_typescript_files;
pub use parse::{ParsedClass, ParsedFile, ParsedFunction, ParsedMethod, parse_file};
pub use recover::recover_operations;
pub use render::{render_files, render_slice};
pub use source::read_typescript_tree;
pub use validate::validate_typescript_files;

/// The TypeScript implementation of [`LanguagePlugin`]. A zero-sized handle that routes the
/// engine's render/import/diff/validate calls to this crate's TypeScript-specific modules, so
/// `core` and `storage` never depend on TypeScript directly.
#[derive(Clone, Copy, Debug, Default)]
pub struct TypeScriptPlugin;

impl LanguagePlugin for TypeScriptPlugin {
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
        import_typescript_files(files)
    }

    fn diff(&self, original: &[RenderedFile], modified: &[RenderedFile]) -> Result<Vec<Operation>> {
        diff_slice(original, modified)
    }

    fn recover_operations(
        &self,
        base: &SemanticGraph,
        scope: &[Uuid],
        edited: &[RenderedFile],
    ) -> Result<Vec<Operation>> {
        recover_operations(base, scope, edited)
    }

    fn read_source_tree(&self, root: &Path) -> Result<Vec<RenderedFile>> {
        read_typescript_tree(root)
    }

    fn validate<'a>(&'a self, files: &'a [RenderedFile]) -> ValidateFuture<'a> {
        Box::pin(validate_typescript_files(files))
    }
}
