mod recover;
mod render;

use crate::{
    TypeScriptPlugin, diff_slice, import_typescript_files, read_typescript_tree, render_files,
    validate_typescript_files_with_compiler,
};
use bonhomme_core::{
    Handler, Operation, OperationRecord, RenderedFile, SemanticGraph, materialize, metadata_string,
};
use chrono::{TimeZone, Utc};
use serde_json::json;
use std::fs;
use uuid::Uuid;

fn record(position: i64, operation: Operation) -> OperationRecord {
    OperationRecord {
        id: Uuid::new_v4(),
        repository_id: Uuid::nil(),
        branch_id: Uuid::nil(),
        changeset_id: Uuid::nil(),
        position,
        operation,
        created_at: Utc.timestamp_opt(0, 0).unwrap(),
    }
}

fn path_only(path: &str) -> RenderedFile {
    RenderedFile {
        path: path.to_string(),
        content: String::new(),
    }
}

async fn validate_with_repo_compiler(files: &[RenderedFile]) {
    let compiler = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("demo")
        .join("node_modules")
        .join(".bin")
        .join("tsc");
    validate_typescript_files_with_compiler(files, compiler.to_str())
        .await
        .unwrap();
}

#[test]
fn handler_claims_typescript_and_javascript_family_sources() {
    let plugin = TypeScriptPlugin::default();

    for path in [
        "src/index.ts",
        "src/component.tsx",
        "src/util.js",
        "src/view.jsx",
    ] {
        assert!(plugin.claims(&path_only(path)), "{path} should be claimed");
    }

    for path in ["src/types.d.ts", "package.json", "src/service.go"] {
        assert!(
            !plugin.claims(&path_only(path)),
            "{path} should not be claimed"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn configured_compiler_is_used_for_validation() {
    if std::env::var_os("BONHOMME_TSC").is_some() {
        return;
    }

    use std::os::unix::fs::PermissionsExt;

    fn shell_quote(path: &std::path::Path) -> String {
        format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
    }

    let root = std::env::temp_dir().join(format!("bonhomme-ts-toolchain-{}", Uuid::new_v4()));
    let compiler = root.join("fake-tsc");
    let marker = root.join("used");
    fs::create_dir_all(&root).unwrap();
    fs::write(
        &compiler,
        format!("#!/bin/sh\ntouch {}\n", shell_quote(&marker)),
    )
    .unwrap();
    let mut permissions = fs::metadata(&compiler).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&compiler, permissions).unwrap();

    validate_typescript_files_with_compiler(&[], compiler.to_str())
        .await
        .unwrap();

    assert!(marker.exists(), "configured compiler should have run");
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn read_source_tree_includes_typescript_and_javascript_family_sources() {
    let root = std::env::temp_dir().join(format!("bonhomme-ts-source-{}", Uuid::new_v4()));
    let src = root.join("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(
        src.join("index.ts"),
        "export function index(): number { return 1; }\n",
    )
    .unwrap();
    fs::write(
        src.join("component.tsx"),
        "export function Component() { return <div />; }\n",
    )
    .unwrap();
    fs::write(
        src.join("util.js"),
        "export function util() { return 1; }\n",
    )
    .unwrap();
    fs::write(
        src.join("view.jsx"),
        "export function View() { return <div />; }\n",
    )
    .unwrap();
    fs::write(src.join("types.d.ts"), "declare const ignored: string;\n").unwrap();
    fs::write(root.join("package.json"), "{\"name\":\"demo\"}\n").unwrap();

    let files = read_typescript_tree(&root).unwrap();
    let paths = files.into_iter().map(|file| file.path).collect::<Vec<_>>();

    fs::remove_dir_all(&root).unwrap();

    assert_eq!(
        paths,
        vec![
            "src/component.tsx".to_string(),
            "src/index.ts".to_string(),
            "src/util.js".to_string(),
            "src/view.jsx".to_string(),
        ]
    );
}

#[test]
fn import_allows_duplicate_file_basenames() {
    let files = vec![
        RenderedFile {
            path: "extensions/a/src/extension.ts".to_string(),
            content: "export function activateA(): void {}\n".to_string(),
        },
        RenderedFile {
            path: "extensions/b/src/extension.ts".to_string(),
            content: "export function activateB(): void {}\n".to_string(),
        },
    ];

    let operations = import_typescript_files(&files).unwrap();
    let records = operations
        .into_iter()
        .enumerate()
        .map(|(index, operation)| record(index as i64 + 1, operation))
        .collect::<Vec<_>>();
    let graph = materialize(&records).unwrap();
    let file_names = graph
        .root_symbols()
        .into_iter()
        .map(|symbol| symbol.name.clone())
        .collect::<Vec<_>>();

    assert_eq!(
        file_names,
        vec![
            "extensions/a/src/extension.ts".to_string(),
            "extensions/b/src/extension.ts".to_string(),
        ]
    );
}

#[test]
fn render_keeps_shebang_at_start_of_file() {
    let operations = import_typescript_files(&[RenderedFile {
        path: "scripts/run.ts".to_string(),
        content: "#!/usr/bin/env node\n\nexport function main(): void {}\n".to_string(),
    }])
    .unwrap();
    let records = operations
        .into_iter()
        .enumerate()
        .map(|(index, operation)| record(index as i64 + 1, operation))
        .collect::<Vec<_>>();
    let graph = materialize(&records).unwrap();
    let rendered = render_files(&graph);

    assert!(
        rendered[0].content.starts_with("#!/usr/bin/env node\n"),
        "{}",
        rendered[0].content
    );
}

#[tokio::test]
async fn import_typescript_files_round_trips_common_symbols() {
    let files = vec![RenderedFile {
        path: "src/OrderService.ts".to_string(),
        content: r#"
type OrderId = string;

export function formatOrder(id: OrderId): string {
  return id.toUpperCase();
}

export class OrderService {
  private prefix: string = "order";

  displayName(id: OrderId): string {
    return formatOrder(id);
  }

  summary(id: OrderId): string {
    return this.displayName(id);
  }
}
"#
        .to_string(),
    }];
    let operations = import_typescript_files(&files).unwrap();
    let records = operations
        .into_iter()
        .enumerate()
        .map(|(index, operation)| record(index as i64 + 1, operation))
        .collect::<Vec<_>>();
    let graph = materialize(&records).unwrap();
    let rendered = render_files(&graph);

    assert!(rendered[0].content.contains("type OrderId = string;"));
    assert!(rendered[0].content.contains("private prefix: string"));
    assert!(rendered[0].content.contains("formatOrder"));
    assert_eq!(graph.find_symbol("OrderService").len(), 1);
    assert_eq!(graph.find_symbol("displayName").len(), 1);
    let display_name_id = graph.find_symbol("displayName")[0].id;
    assert_eq!(graph.find_callers(display_name_id, "calls").len(), 1);
    assert_eq!(graph.find_callees(display_name_id, "calls").len(), 1);

    validate_with_repo_compiler(&rendered).await;
}

#[tokio::test]
async fn import_javascript_and_tsx_files_round_trips_common_symbols() {
    let files = vec![
        RenderedFile {
            path: "src/math.js".to_string(),
            content: r#"
export function double(value) {
  return value * 2;
}

export class Counter {
  count = 0;

  inc() {
    return double(this.count);
  }
}
"#
            .to_string(),
        },
        RenderedFile {
            path: "src/Badge.tsx".to_string(),
            content: r#"
export function Badge(props: { label: string }) {
  return <span>{props.label}</span>;
}
"#
            .to_string(),
        },
    ];
    let operations = import_typescript_files(&files).unwrap();
    let records = operations
        .into_iter()
        .enumerate()
        .map(|(index, operation)| record(index as i64 + 1, operation))
        .collect::<Vec<_>>();
    let graph = materialize(&records).unwrap();
    let rendered = render_files(&graph);

    assert_eq!(graph.find_symbol("src/math.js").len(), 1);
    assert_eq!(graph.find_symbol("double").len(), 1);
    assert_eq!(graph.find_symbol("Counter").len(), 1);
    assert_eq!(graph.find_symbol("inc").len(), 1);
    assert_eq!(graph.find_symbol("Badge").len(), 1);
    assert!(
        rendered
            .iter()
            .any(|file| file.path == "src/Badge.tsx" && file.content.contains("<span>"))
    );

    validate_with_repo_compiler(&rendered).await;
}

#[tokio::test]
async fn diff_slice_imports_new_files_as_create_operations() {
    let modified = vec![RenderedFile {
        path: "src/Inventory.ts".to_string(),
        content: r#"
export function describeInventory(count: number): string {
  return `inventory:${count}`;
}
"#
        .to_string(),
    }];
    let operations = diff_slice(&[], &modified).unwrap();
    let records = operations
        .into_iter()
        .enumerate()
        .map(|(index, operation)| record(index as i64 + 1, operation))
        .collect::<Vec<_>>();
    let graph = materialize(&records).unwrap();
    let rendered = render_files(&graph);

    assert_eq!(graph.find_symbol("src/Inventory.ts").len(), 1);
    assert_eq!(graph.find_symbol("describeInventory").len(), 1);
    assert!(rendered[0].content.contains("describeInventory"));

    validate_with_repo_compiler(&rendered).await;
}

#[tokio::test]
async fn diff_slice_updates_and_deletes_top_level_functions() {
    let file_id = Uuid::new_v4();
    let format_id = Uuid::new_v4();
    let unused_id = Uuid::new_v4();
    let original = vec![RenderedFile {
        path: "src/format.ts".to_string(),
        content: format!(
            r#"// bonhomme:file={file_id}

export function formatOrder(id: string): string /* bonhomme:symbol={format_id} */ {{
  return id;
}}

export function unused(): string /* bonhomme:symbol={unused_id} */ {{
  return "unused";
}}
"#
        ),
    }];
    let modified = vec![RenderedFile {
        path: "src/format.ts".to_string(),
        content: format!(
            r#"// bonhomme:file={file_id}

export function formatOrder(id: string): string /* bonhomme:symbol={format_id} */ {{
  return id.toUpperCase();
}}
"#
        ),
    }];
    let operations = diff_slice(&original, &modified).unwrap();

    assert!(matches!(
        operations[0],
        Operation::DeleteSymbol {
            symbol_id
        } if symbol_id == unused_id
    ));
    assert!(matches!(
        operations[1],
        Operation::UpdateSymbol {
            symbol_id,
            ..
        } if symbol_id == format_id
    ));

    let mut records = vec![
        record(
            1,
            Operation::CreateSymbol {
                symbol_id: file_id,
                parent_id: None,
                kind: "file".to_string(),
                name: "format.ts".to_string(),
                body: None,
                metadata: json!({"path": "src/format.ts"}),
            },
        ),
        record(
            2,
            Operation::CreateSymbol {
                symbol_id: format_id,
                parent_id: Some(file_id),
                kind: "function".to_string(),
                name: "formatOrder".to_string(),
                body: Some("return id;".to_string()),
                metadata: json!({
                    "declaration": "export function formatOrder(id: string): string",
                    "exported": true
                }),
            },
        ),
        record(
            3,
            Operation::CreateSymbol {
                symbol_id: unused_id,
                parent_id: Some(file_id),
                kind: "function".to_string(),
                name: "unused".to_string(),
                body: Some("return \"unused\";".to_string()),
                metadata: json!({
                    "declaration": "export function unused(): string",
                    "exported": true
                }),
            },
        ),
    ];
    records.extend(
        operations
            .into_iter()
            .enumerate()
            .map(|(index, operation)| record(index as i64 + 4, operation)),
    );
    let graph = materialize(&records).unwrap();
    let rendered = render_files(&graph);

    assert_eq!(graph.find_symbol("unused").len(), 0);
    assert!(rendered[0].content.contains("id.toUpperCase()"));

    validate_with_repo_compiler(&rendered).await;
}

fn import_graph(content: &str) -> SemanticGraph {
    materialize_operations(import_operations(content))
}

fn import_operations(content: &str) -> Vec<Operation> {
    let files = vec![sample_file(content)];
    import_typescript_files(&files).unwrap()
}

fn materialize_operations(operations: Vec<Operation>) -> SemanticGraph {
    let records = operations
        .into_iter()
        .enumerate()
        .map(|(index, operation)| record(index as i64 + 1, operation))
        .collect::<Vec<_>>();
    materialize(&records).unwrap()
}

fn sample_file(content: &str) -> RenderedFile {
    RenderedFile {
        path: "src/Sample.ts".to_string(),
        content: content.to_string(),
    }
}

#[test]
fn import_does_not_capture_control_flow_blocks_as_methods() {
    let graph = import_graph(
        r#"
export class Worker {
  run(ready: boolean): number {
    if (ready) {
      return 1;
    }
    for (let i = 0; i < 3; i = i + 1) {
      this.run(ready);
    }
    return 0;
  }
}
"#,
    );

    let methods = graph
        .symbols
        .values()
        .filter(|symbol| symbol.kind == "method")
        .collect::<Vec<_>>();
    assert_eq!(methods.len(), 1, "only the real method should be imported");
    assert_eq!(methods[0].name, "run");
    assert!(graph.find_symbol("if").is_empty());
    assert!(graph.find_symbol("for").is_empty());
}

#[test]
fn import_handles_callback_typed_parameters() {
    let graph = import_graph(
        r#"
export class Events {
  on(handler: (value: number) => void): void {
    handler(value);
  }
}

export function subscribe(cb: (n: number) => void): void {
  cb(2);
}
"#,
    );

    assert_eq!(graph.find_symbol("on").len(), 1);
    assert_eq!(graph.find_symbol("subscribe").len(), 1);
}

#[test]
fn import_handles_generic_object_type_in_heritage() {
    let graph = import_graph(
        r#"
export class Store extends Base<{ id: number }> {
  size(): number {
    return 0;
  }
}
"#,
    );

    assert_eq!(graph.find_symbol("Store").len(), 1);
    assert_eq!(graph.find_symbol("size").len(), 1);
    let store = graph.find_symbol("Store")[0];
    assert!(
        metadata_string(&store.metadata, "declaration")
            .unwrap()
            .contains("extends Base<{ id: number }>")
    );
}

#[test]
fn diff_matches_comment_dropped_edits_by_name() {
    let file_id = Uuid::new_v4();
    let format_id = Uuid::new_v4();
    let original = vec![RenderedFile {
        path: "src/format.ts".to_string(),
        content: format!(
            r#"// bonhomme:file={file_id}

export function formatOrder(id: string): string /* bonhomme:symbol={format_id} */ {{
  return id;
}}
"#
        ),
    }];
    let modified = vec![RenderedFile {
        path: "src/format.ts".to_string(),
        content: format!(
            r#"// bonhomme:file={file_id}

export function formatOrder(id: string): string {{
  return id.trim();
}}
"#
        ),
    }];

    let operations = diff_slice(&original, &modified).unwrap();

    // A dropped symbol comment on an otherwise-unchanged-identity edit must stay an
    // UpdateSymbol that preserves the original id, never a Delete + Create.
    assert_eq!(operations.len(), 1);
    assert!(matches!(
        &operations[0],
        Operation::UpdateSymbol { symbol_id, .. } if *symbol_id == format_id
    ));
}

#[test]
fn diff_added_method_id_is_deterministic() {
    let file_id = Uuid::new_v4();
    let class_id = Uuid::new_v4();
    let existing_id = Uuid::new_v4();
    let original = vec![RenderedFile {
        path: "src/Svc.ts".to_string(),
        content: format!(
            r#"// bonhomme:file={file_id}

class Svc /* bonhomme:symbol={class_id} */ {{
  existing(): void /* bonhomme:symbol={existing_id} */ {{
    return;
  }}
}}
"#
        ),
    }];
    let modified = vec![RenderedFile {
        path: "src/Svc.ts".to_string(),
        content: format!(
            r#"// bonhomme:file={file_id}

class Svc /* bonhomme:symbol={class_id} */ {{
  existing(): void /* bonhomme:symbol={existing_id} */ {{
    return;
  }}
  added(): void {{
    return;
  }}
}}
"#
        ),
    }];

    let first = diff_slice(&original, &modified).unwrap();
    let second = diff_slice(&original, &modified).unwrap();
    assert_eq!(first, second, "diff must be deterministic across runs");
    assert!(matches!(
        first.as_slice(),
        [Operation::CreateSymbol { name, .. }] if name == "added"
    ));
}

#[test]
fn diff_rejects_slices_that_reuse_a_symbol_id() {
    let file_id = Uuid::new_v4();
    let class_id = Uuid::new_v4();
    let method_id = Uuid::new_v4();
    let original = vec![RenderedFile {
        path: "src/Svc.ts".to_string(),
        content: format!(
            r#"// bonhomme:file={file_id}

class Svc /* bonhomme:symbol={class_id} */ {{
  existing(): void /* bonhomme:symbol={method_id} */ {{
    return;
  }}
}}
"#
        ),
    }];
    // The agent copy-pasted a method along with its identity comment, so the same id appears twice.
    let modified = vec![RenderedFile {
        path: "src/Svc.ts".to_string(),
        content: format!(
            r#"// bonhomme:file={file_id}

class Svc /* bonhomme:symbol={class_id} */ {{
  existing(): void /* bonhomme:symbol={method_id} */ {{
    return;
  }}
  existingCopy(): void /* bonhomme:symbol={method_id} */ {{
    return;
  }}
}}
"#
        ),
    }];

    let error = diff_slice(&original, &modified)
        .expect_err("a reused symbol id must be rejected")
        .to_string();
    assert!(
        error.contains(&method_id.to_string()) && error.contains("must be unique"),
        "expected a duplicate-id rejection, got: {error}"
    );
}
