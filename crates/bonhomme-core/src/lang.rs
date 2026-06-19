use crate::core::{Operation, SemanticGraph};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{future::Future, path::Path, pin::Pin};
use uuid::Uuid;

/// A rendered source file: a path and its textual content. This is the compatibility bridge
/// between the semantic graph and the file-shaped tooling a language expects; it carries no
/// language-specific structure of its own.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RenderedFile {
    pub path: String,
    pub content: String,
}

/// An editable projection of part of the repository handed to an agent. The agent edits the
/// rendered files; the diff back into operations is what the system actually records.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Slice {
    pub id: Uuid,
    pub base_revision: String,
    pub root_symbols: Vec<Uuid>,
    pub files: Vec<RenderedFile>,
}

/// The future returned by [`LanguagePlugin::validate`]. Returned boxed (rather than via an
/// `async fn`) so `LanguagePlugin` stays object-safe and can be held as `dyn LanguagePlugin`.
pub type ValidateFuture<'a> = Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>>;

/// The pluggable boundary between the language-agnostic operation/graph core and a concrete
/// language. The core (operations, semantic graph, replay, validation, merge analysis) and the
/// storage/merge engine know nothing about any particular language; they render, validate, import,
/// and diff exclusively through this trait. Adding a new language means implementing this trait and
/// wiring it in at the composition root — no changes to `core` or `storage`.
pub trait LanguagePlugin: Send + Sync {
    /// Render the whole semantic graph into source files.
    fn render(&self, graph: &SemanticGraph) -> Vec<RenderedFile>;

    /// Render a focused, editable slice of the graph around the requested root symbols.
    fn render_slice(
        &self,
        graph: &SemanticGraph,
        base_revision: String,
        root_symbols: Vec<Uuid>,
    ) -> Slice;

    /// Parse source files into the operations that would reconstruct them.
    fn import(&self, files: &[RenderedFile]) -> Result<Vec<Operation>>;

    /// Diff an edited slice against its original projection, producing operations.
    fn diff(&self, original: &[RenderedFile], modified: &[RenderedFile]) -> Result<Vec<Operation>>;

    /// Read a source tree from disk into rendered files, applying the language's file conventions
    /// (extensions, ignored directories, generated files).
    fn read_source_tree(&self, root: &Path) -> Result<Vec<RenderedFile>>;

    /// Validate rendered files with the language toolchain (for TypeScript, the compiler). Acts as
    /// an external validator for the rendered projection after replay and merge.
    fn validate<'a>(&'a self, files: &'a [RenderedFile]) -> ValidateFuture<'a>;
}
