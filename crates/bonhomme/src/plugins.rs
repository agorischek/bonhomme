use std::sync::Arc;

use bonhomme_core::{BlobHandler, Handler, HandlerRegistry, LanguagePlugin};
use bonhomme_csharp::CSharpPlugin;
use bonhomme_elixir::ElixirPlugin;
use bonhomme_fallback::{
    JsonHandler, MarkdownHandler, TomlHandler, TreeSitterHandler, YamlHandler,
};
use bonhomme_go::GoPlugin;
use bonhomme_python::PythonPlugin;
use bonhomme_rust::RustPlugin;
use bonhomme_ts::TypeScriptPlugin;

use crate::config::Config;

/// Build the per-file handler router injected into [`bonhomme_engine::Storage`]. This is the one
/// composition root for languages: the storage/merge engine holds a single `Arc<dyn LanguagePlugin>`
/// (the registry) and never grows a "no plugin" branch.
///
/// Order is priority order — the most specific handlers claim first, and the blob handler is
/// terminal, claiming everything as the universal floor. New language plugins slot in ahead of the
/// blob handler.
pub fn language_registry(config: &Config) -> Arc<dyn LanguagePlugin> {
    Arc::new(handler_registry(config))
}

/// The concrete registry behind [`language_registry`]. Kept separate so tests can call
/// [`HandlerRegistry`] methods (e.g. the handler breakdown) that the `dyn LanguagePlugin` view hides.
fn handler_registry(config: &Config) -> HandlerRegistry {
    HandlerRegistry::new(vec![
        Arc::new(TypeScriptPlugin::with_compiler(typescript_compiler(config))) as Arc<dyn Handler>,
        Arc::new(GoPlugin),
        Arc::new(RustPlugin),
        Arc::new(PythonPlugin::with_interpreter(python_interpreter(config))),
        Arc::new(CSharpPlugin::with_dotnet(dotnet_binary(config))),
        Arc::new(ElixirPlugin::with_compiler(elixir_compiler(config))),
        Arc::new(JsonHandler),
        Arc::new(MarkdownHandler),
        Arc::new(TomlHandler),
        Arc::new(YamlHandler),
        // Tree-sitter is the broad structural-lite tier for grammars that have not graduated to a
        // full plugin; it sits just above the terminal blob floor.
        Arc::new(TreeSitterHandler),
        Arc::new(BlobHandler),
    ])
}

fn typescript_compiler(config: &Config) -> Option<String> {
    config
        .toolchain
        .get("typescript")
        .or_else(|| config.toolchain.get("tsc"))
        .cloned()
}

fn python_interpreter(config: &Config) -> Option<String> {
    config
        .toolchain
        .get("python")
        .or_else(|| config.toolchain.get("python3"))
        .cloned()
}

fn dotnet_binary(config: &Config) -> Option<String> {
    config.toolchain.get("dotnet").cloned()
}

fn elixir_compiler(config: &Config) -> Option<String> {
    config
        .toolchain
        .get("elixirc")
        .or_else(|| config.toolchain.get("elixir"))
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bonhomme_core::{
        LanguagePlugin, Operation, OperationRecord, RenderedFile, SemanticGraph, decode_binary,
        encode_binary, materialize,
    };
    use std::collections::BTreeMap;
    use uuid::Uuid;

    fn rf(path: &str, content: &str) -> RenderedFile {
        RenderedFile {
            path: path.to_string(),
            content: content.to_string(),
        }
    }

    fn config_with_toolchain(key: &str, value: &str) -> Config {
        let mut config = Config::default();
        config.toolchain.insert(key.to_string(), value.to_string());
        config
    }

    fn graph_from(operations: &[Operation]) -> SemanticGraph {
        let records = operations
            .iter()
            .enumerate()
            .map(|(index, operation)| OperationRecord {
                id: Uuid::new_v4(),
                repository_id: Uuid::nil(),
                branch_id: Uuid::nil(),
                changeset_id: Uuid::nil(),
                position: index as i64 + 1,
                operation: operation.clone(),
                created_at: chrono::DateTime::<chrono::Utc>::UNIX_EPOCH,
            })
            .collect::<Vec<_>>();
        materialize(&records).expect("polyglot operations materialize into a valid graph")
    }

    #[test]
    fn polyglot_repo_routes_renders_and_round_trips_every_tier() {
        let registry = handler_registry(&Config::default());
        let binary = encode_binary(&[0xFFu8, 0x00, 0x10, 0xAB]);
        let files = vec![
            rf(
                "src/app.ts",
                "export function f(): number {\n  return 1;\n}\n",
            ),
            rf("src/lib.rs", "pub fn answer() -> usize {\n    42\n}\n"),
            rf(
                "src/Service.cs",
                "class Service {\n    int Answer() {\n        return 42;\n    }\n}\n",
            ),
            rf(
                "lib/service.ex",
                "defmodule Demo.Service do\n  def answer do\n    42\n  end\nend\n",
            ),
            rf("package.json", "{\"name\":\"demo\"}"),
            rf("README.md", "# Title\n\nsome text\n"),
            rf("Cargo.toml", "[package]\nname = \"demo\"\n"),
            rf("util.py", "def greet():\n    return 1\n"),
            rf("LICENSE", "MIT, verbatim.\n"),
            rf("logo.png", &binary),
        ];

        let operations = registry.import(&files).expect("polyglot import succeeds");
        let graph = graph_from(&operations);

        // Each file routed to the right handler; LICENSE and the binary are the two opaque blobs.
        let breakdown = registry.handler_breakdown(&graph);
        assert_eq!(breakdown.get("typescript"), Some(&1), "{breakdown:?}");
        assert_eq!(breakdown.get("rust"), Some(&1), "{breakdown:?}");
        assert_eq!(breakdown.get("python"), Some(&1), "{breakdown:?}");
        assert_eq!(breakdown.get("csharp"), Some(&1), "{breakdown:?}");
        assert_eq!(breakdown.get("elixir"), Some(&1), "{breakdown:?}");
        assert_eq!(breakdown.get("json"), Some(&1), "{breakdown:?}");
        assert_eq!(breakdown.get("markdown"), Some(&1), "{breakdown:?}");
        assert_eq!(breakdown.get("toml"), Some(&1), "{breakdown:?}");
        assert_eq!(breakdown.get("blob"), Some(&2), "{breakdown:?}");

        // Render the whole graph back through the router; span-preserving tiers are byte-identical
        // and the binary decodes to its original bytes.
        let rendered = registry.render(&graph);
        let by_path: BTreeMap<&str, &str> = rendered
            .iter()
            .map(|file| (file.path.as_str(), file.content.as_str()))
            .collect();
        assert_eq!(by_path["README.md"], "# Title\n\nsome text\n");
        assert_eq!(by_path["Cargo.toml"], "[package]\nname = \"demo\"\n");
        assert_eq!(by_path["util.py"], "def greet():\n    return 1\n");
        assert!(by_path["src/Service.cs"].contains("class Service"));
        assert!(by_path["lib/service.ex"].contains("defmodule Demo.Service"));
        assert_eq!(by_path["LICENSE"], "MIT, verbatim.\n");
        assert_eq!(
            decode_binary(by_path["logo.png"]).as_deref(),
            Some(&[0xFFu8, 0x00, 0x10, 0xAB][..])
        );
    }

    #[test]
    fn documentation_comments_survive_round_trip_for_every_language() {
        // The cross-plugin doc-fidelity gate: every structural language must round-trip its
        // documentation convention. A plugin that parses code but forgets to model its doc comments
        // silently loses API documentation (TypeScript TSDoc and Go godoc both did) — this fails
        // loudly so a future plugin (or a regression) cannot ship that gap unseen.
        let registry = handler_registry(&Config::default());
        let cases = [
            (
                "a.ts",
                "/** ts-doc-marker */\nexport function f(): number {\n  return 1;\n}\n",
                "ts-doc-marker",
            ),
            ("a.go", "package p\n\n// go-doc-marker\nfunc F() {}\n", "go-doc-marker"),
            (
                "a.rs",
                "/// rs-doc-marker\npub fn answer() -> usize {\n    1\n}\n",
                "rs-doc-marker",
            ),
            (
                "a.py",
                "def greet():\n    \"\"\"py-doc-marker\"\"\"\n    return 1\n",
                "py-doc-marker",
            ),
            (
                "Svc.cs",
                "class Svc {\n    /// <summary>cs-doc-marker</summary>\n    int M() {\n        return 1;\n    }\n}\n",
                "cs-doc-marker",
            ),
            (
                "svc.ex",
                "defmodule Svc do\n  @moduledoc \"ex-doc-marker\"\nend\n",
                "ex-doc-marker",
            ),
        ];

        let files: Vec<RenderedFile> = cases.iter().map(|(path, src, _)| rf(path, src)).collect();
        let operations = registry.import(&files).expect("polyglot doc import succeeds");
        let graph = graph_from(&operations);

        // Guard: each fixture must parse *structurally*, not degrade to blob — otherwise docs would
        // survive verbatim and the gate would pass without testing the plugin's doc handling.
        let breakdown = registry.handler_breakdown(&graph);
        for handler in ["typescript", "go", "rust", "python", "csharp", "elixir"] {
            assert_eq!(
                breakdown.get(handler),
                Some(&1),
                "{handler} fixture did not parse structurally (degraded to blob): {breakdown:?}"
            );
        }

        let rendered = registry.render(&graph);
        let by_path: BTreeMap<&str, &str> = rendered
            .iter()
            .map(|file| (file.path.as_str(), file.content.as_str()))
            .collect();

        for (path, _, marker) in cases {
            let content = by_path
                .get(path)
                .unwrap_or_else(|| panic!("{path} was not rendered"));
            assert!(
                content.contains(marker),
                "{path}: documentation comment '{marker}' was dropped on round-trip:\n{content}"
            );
        }
    }

    #[test]
    fn unparseable_structured_file_degrades_to_blob() {
        let registry = handler_registry(&Config::default());
        // A `.json` extension but malformed contents: the JSON handler errors, and the router
        // degrades just this file to the blob floor rather than failing the whole import.
        let operations = registry
            .import(&[rf("broken.json", "{ this is not json ")])
            .expect("import degrades rather than failing");
        let graph = graph_from(&operations);
        assert_eq!(registry.handler_breakdown(&graph).get("blob"), Some(&1));
    }

    #[test]
    fn typescript_toolchain_comes_from_config() {
        let config = config_with_toolchain("typescript", "tsgo");

        assert_eq!(typescript_compiler(&config).as_deref(), Some("tsgo"));
    }

    #[test]
    fn tsc_toolchain_key_is_an_alias_for_typescript() {
        let config = config_with_toolchain("tsc", "custom-tsc");

        assert_eq!(typescript_compiler(&config).as_deref(), Some("custom-tsc"));
    }

    #[test]
    fn typescript_toolchain_key_takes_precedence_over_tsc_alias() {
        let mut config = config_with_toolchain("tsc", "custom-tsc");
        config
            .toolchain
            .insert("typescript".to_string(), "tsgo".to_string());

        assert_eq!(typescript_compiler(&config).as_deref(), Some("tsgo"));
    }

    #[test]
    fn python_toolchain_comes_from_config() {
        let config = config_with_toolchain("python", "/opt/python");

        assert_eq!(python_interpreter(&config).as_deref(), Some("/opt/python"));
    }

    #[test]
    fn python3_toolchain_key_is_an_alias_for_python() {
        let config = config_with_toolchain("python3", "/opt/python3");

        assert_eq!(python_interpreter(&config).as_deref(), Some("/opt/python3"));
    }

    #[test]
    fn dotnet_toolchain_comes_from_config() {
        let config = config_with_toolchain("dotnet", "/opt/dotnet");

        assert_eq!(dotnet_binary(&config).as_deref(), Some("/opt/dotnet"));
    }

    #[test]
    fn elixirc_toolchain_comes_from_config() {
        let config = config_with_toolchain("elixirc", "/opt/elixirc");

        assert_eq!(elixir_compiler(&config).as_deref(), Some("/opt/elixirc"));
    }

    #[test]
    fn elixir_toolchain_key_is_an_alias_for_elixirc() {
        let config = config_with_toolchain("elixir", "/opt/elixir");

        assert_eq!(elixir_compiler(&config).as_deref(), Some("/opt/elixir"));
    }
}
