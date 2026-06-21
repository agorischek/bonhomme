mod binary;
mod blob;
mod registry;

pub use binary::{MAX_INLINE_BINARY_BYTES, decode_binary, encode_binary, is_binary};
pub use blob::BlobHandler;
pub use registry::{Handler, HandlerRegistry, read_source_files};

use crate::core::{Operation, SemanticGraph};
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    future::Future,
    path::{Component, Path, PathBuf},
    pin::Pin,
};
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

/// Convert a rendered source path into a filesystem-relative path after rejecting traversal and
/// platform-specific absolute forms. `RenderedFile.path` is serialized, stored, and sometimes read
/// from user-edited JSON, so disk writers and validators must not join it directly onto an output
/// root.
pub fn safe_relative_path(path: &str) -> Result<PathBuf> {
    if path.is_empty() {
        bail!("rendered file path must not be empty");
    }
    if path.contains('\\') {
        bail!("rendered file path must use forward slashes: {path}");
    }

    let mut safe = PathBuf::new();
    for component in Path::new(path).components() {
        match component {
            Component::Normal(part) => {
                let part = part.to_string_lossy();
                if part.is_empty() || part == "." || part == ".." || part.ends_with(':') {
                    bail!("rendered file path contains an unsafe component: {path}");
                }
                safe.push(part.as_ref());
            }
            Component::CurDir
            | Component::ParentDir
            | Component::RootDir
            | Component::Prefix(_) => {
                bail!("rendered file path must be relative and normalized: {path}");
            }
        }
    }

    if safe.as_os_str().is_empty() {
        bail!("rendered file path must not be empty");
    }
    Ok(safe)
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

/// Repo-shaped validation input. `files` is the subset owned by the current handler; `all_files` is
/// the full rendered repository tree. Real language toolchains often need workspace files they do
/// not semantically own, such as `go.mod`, lockfiles, config files, or embedded assets.
#[derive(Clone, Copy, Debug)]
pub struct ValidationContext<'a> {
    pub all_files: &'a [RenderedFile],
    pub files: &'a [RenderedFile],
}

impl<'a> ValidationContext<'a> {
    pub fn new(all_files: &'a [RenderedFile], files: &'a [RenderedFile]) -> Self {
        Self { all_files, files }
    }
}

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

    /// Recover operations from edited source by matching it against an authoritative graph
    /// snapshot. This is the graph-anchored structural path used when slice provenance is
    /// available; `diff` remains the legacy two-blob path.
    fn recover_operations(
        &self,
        base: &SemanticGraph,
        scope: &[Uuid],
        edited: &[RenderedFile],
    ) -> Result<Vec<Operation>>;

    /// Read a source tree from disk into rendered files, applying the language's file conventions
    /// (extensions, ignored directories, generated files).
    fn read_source_tree(&self, root: &Path) -> Result<Vec<RenderedFile>>;

    /// Validate rendered files with the language toolchain (for TypeScript, the compiler). Acts as
    /// an external validator for the rendered projection after replay and merge.
    fn validate<'a>(&'a self, files: &'a [RenderedFile]) -> ValidateFuture<'a>;

    /// Validate with whole-repo context. Handlers that only need their own files can use the
    /// default; workspace-aware handlers can read `context.all_files`.
    fn validate_with_context<'a>(&'a self, context: ValidationContext<'a>) -> ValidateFuture<'a> {
        self.validate(context.files)
    }
}

#[cfg(test)]
mod tests {
    use super::safe_relative_path;

    #[test]
    fn safe_relative_path_accepts_normal_repo_paths() {
        assert_eq!(
            safe_relative_path("src/components/App.tsx").unwrap(),
            std::path::PathBuf::from("src/components/App.tsx")
        );
    }

    #[test]
    fn safe_relative_path_rejects_escape_paths() {
        for path in [
            "",
            ".",
            "/tmp/out.ts",
            "../out.ts",
            "src/../../out.ts",
            "src\\out.ts",
            "C:/repo/out.ts",
        ] {
            assert!(safe_relative_path(path).is_err(), "{path} should fail");
        }
    }
}
