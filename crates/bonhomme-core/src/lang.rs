mod binary;
mod blob;
mod registry;

pub use binary::{MAX_INLINE_BINARY_BYTES, decode_binary, encode_binary, is_binary};
pub use blob::BlobHandler;
pub use registry::{Handler, HandlerManifest, HandlerRegistry, read_source_files};

use crate::core::{Operation, SemanticGraph};
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    future::Future,
    path::{Component, Path, PathBuf},
    pin::Pin,
};
use uuid::Uuid;

use crate::core::ProtocolRange;

/// Evidence level for a plugin feature. `Verified` is reserved for capabilities covered by that
/// plugin's conformance tests; `Supported` means implemented but not yet covered by the shared
/// conformance corpus. This prevents a single "supports language X" boolean from overstating
/// semantic coverage.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub enum CapabilityLevel {
    Undeclared,
    Unsupported,
    Experimental,
    Supported,
    Verified,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SourceFidelity {
    Undeclared,
    /// The handler preserves the exact file payload (including Bonhomme's binary envelope).
    ByteExact,
    /// Rendering is deterministic and semantically equivalent but may normalize formatting.
    SemanticCanonical,
    /// Only the structures claimed by the handler are preserved.
    Structural,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub enum SymbolGranularity {
    Undeclared,
    File,
    TopLevel,
    Member,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum IdentityStability {
    Undeclared,
    None,
    /// Fresh imports are deterministic while the repository-relative path remains stable.
    StableWithinPath,
    /// Graph-anchored recovery preserves identity across supported renames and moves.
    GraphAnchored,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub enum ValidationStrength {
    Undeclared,
    None,
    Parser,
    Syntax,
    Compiler,
    WorkspaceCompiler,
}

/// A serializable capability declaration. It is a protocol-facing promise, not marketing copy:
/// consumers can gate automation on individual fields and retain the exact report used for a merge.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PluginCapabilities {
    pub protocol: ProtocolRange,
    pub source_fidelity: SourceFidelity,
    pub symbol_granularity: SymbolGranularity,
    pub identity_stability: IdentityStability,
    pub reference_extraction: CapabilityLevel,
    pub rename_recovery: CapabilityLevel,
    pub move_recovery: CapabilityLevel,
    pub validation: ValidationStrength,
    pub incremental_parsing: CapabilityLevel,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub known_limitations: Vec<String>,
}

impl PluginCapabilities {
    /// Safe default for third-party plugins compiled before capability reporting existed.
    pub fn undeclared() -> Self {
        Self {
            protocol: ProtocolRange::current(),
            source_fidelity: SourceFidelity::Undeclared,
            symbol_granularity: SymbolGranularity::Undeclared,
            identity_stability: IdentityStability::Undeclared,
            reference_extraction: CapabilityLevel::Undeclared,
            rename_recovery: CapabilityLevel::Undeclared,
            move_recovery: CapabilityLevel::Undeclared,
            validation: ValidationStrength::Undeclared,
            incremental_parsing: CapabilityLevel::Undeclared,
            known_limitations: vec![
                "plugin has not published a capability declaration".to_string(),
            ],
        }
    }

    /// Baseline for the full semantic plugins in this workspace. Individual plugins can refine the
    /// evidence level or limitations with the builder methods below.
    pub fn semantic(validation: ValidationStrength) -> Self {
        Self {
            protocol: ProtocolRange::current(),
            source_fidelity: SourceFidelity::SemanticCanonical,
            symbol_granularity: SymbolGranularity::Member,
            identity_stability: IdentityStability::GraphAnchored,
            reference_extraction: CapabilityLevel::Supported,
            rename_recovery: CapabilityLevel::Experimental,
            move_recovery: CapabilityLevel::Experimental,
            validation,
            incremental_parsing: CapabilityLevel::Unsupported,
            known_limitations: Vec::new(),
        }
    }

    pub fn structural(
        source_fidelity: SourceFidelity,
        symbol_granularity: SymbolGranularity,
        validation: ValidationStrength,
    ) -> Self {
        Self {
            protocol: ProtocolRange::current(),
            source_fidelity,
            symbol_granularity,
            identity_stability: IdentityStability::StableWithinPath,
            reference_extraction: CapabilityLevel::Unsupported,
            rename_recovery: CapabilityLevel::Experimental,
            move_recovery: CapabilityLevel::Unsupported,
            validation,
            incremental_parsing: CapabilityLevel::Unsupported,
            known_limitations: Vec::new(),
        }
    }

    pub fn with_limitations(mut self, limitations: &[&str]) -> Self {
        self.known_limitations = limitations
            .iter()
            .map(|limitation| (*limitation).to_string())
            .collect();
        self
    }
}

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
    /// Machine-readable semantic coverage. The default is intentionally conservative so adding
    /// this method does not break third-party plugins and an undeclared plugin cannot be mistaken
    /// for a fully semantic one.
    fn capabilities(&self) -> PluginCapabilities {
        PluginCapabilities::undeclared()
    }

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
    use serde_json::json;

    use super::{
        CapabilityLevel, LanguagePlugin, PluginCapabilities, SourceFidelity, ValidationStrength,
        safe_relative_path,
    };
    use crate::core::SUPPORTED_PROTOCOL_RANGE;

    struct UndeclaredPlugin;

    impl LanguagePlugin for UndeclaredPlugin {
        fn render(&self, _: &crate::SemanticGraph) -> Vec<super::RenderedFile> {
            Vec::new()
        }

        fn render_slice(
            &self,
            _: &crate::SemanticGraph,
            base_revision: String,
            root_symbols: Vec<uuid::Uuid>,
        ) -> super::Slice {
            super::Slice {
                id: uuid::Uuid::nil(),
                base_revision,
                root_symbols,
                files: Vec::new(),
            }
        }

        fn import(&self, _: &[super::RenderedFile]) -> anyhow::Result<Vec<crate::Operation>> {
            Ok(Vec::new())
        }

        fn diff(
            &self,
            _: &[super::RenderedFile],
            _: &[super::RenderedFile],
        ) -> anyhow::Result<Vec<crate::Operation>> {
            Ok(Vec::new())
        }

        fn recover_operations(
            &self,
            _: &crate::SemanticGraph,
            _: &[uuid::Uuid],
            _: &[super::RenderedFile],
        ) -> anyhow::Result<Vec<crate::Operation>> {
            Ok(Vec::new())
        }

        fn read_source_tree(
            &self,
            _: &std::path::Path,
        ) -> anyhow::Result<Vec<super::RenderedFile>> {
            Ok(Vec::new())
        }

        fn validate<'a>(&'a self, _: &'a [super::RenderedFile]) -> super::ValidateFuture<'a> {
            Box::pin(async { Ok(()) })
        }
    }

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

    #[test]
    fn undeclared_capabilities_are_safe_and_wire_stable() {
        let capabilities = UndeclaredPlugin.capabilities();
        assert_eq!(capabilities.source_fidelity, SourceFidelity::Undeclared);
        assert_eq!(
            capabilities.reference_extraction,
            CapabilityLevel::Undeclared
        );
        assert_eq!(
            serde_json::to_value(&capabilities).unwrap()["protocol"],
            json!({
                "minimum": SUPPORTED_PROTOCOL_RANGE.minimum,
                "maximum": SUPPORTED_PROTOCOL_RANGE.maximum
            })
        );
    }

    #[test]
    fn semantic_capabilities_expose_independent_measures() {
        let capabilities = PluginCapabilities::semantic(ValidationStrength::Compiler)
            .with_limitations(&["macros are preserved but not expanded"]);
        let value = serde_json::to_value(capabilities).unwrap();
        assert_eq!(value["validation"], "compiler");
        assert_eq!(value["symbolGranularity"], "member");
        assert_eq!(value["referenceExtraction"], "supported");
        assert_eq!(
            value["protocol"]["maximum"],
            serde_json::to_value(SUPPORTED_PROTOCOL_RANGE.maximum).unwrap()
        );
    }
}
