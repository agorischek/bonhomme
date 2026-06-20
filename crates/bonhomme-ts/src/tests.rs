use crate::{diff_slice, import_typescript_files, render_files, validate_typescript_files};
use bonhomme_core::{
    Operation, OperationRecord, RenderedFile, SemanticGraph, materialize, metadata_string,
};
use chrono::{TimeZone, Utc};
use serde_json::json;
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

#[test]
fn render_is_deterministic() {
    let file = Uuid::new_v4();
    let class = Uuid::new_v4();
    let method = Uuid::new_v4();
    let records = vec![
        record(
            1,
            Operation::CreateSymbol {
                symbol_id: file,
                parent_id: None,
                kind: "file".to_string(),
                name: "OrderService.ts".to_string(),
                body: None,
                metadata: json!({"path": "src/OrderService.ts"}),
            },
        ),
        record(
            2,
            Operation::CreateSymbol {
                symbol_id: class,
                parent_id: Some(file),
                kind: "class".to_string(),
                name: "OrderService".to_string(),
                body: None,
                metadata: json!({"exported": true}),
            },
        ),
        record(
            3,
            Operation::CreateSymbol {
                symbol_id: method,
                parent_id: Some(class),
                kind: "method".to_string(),
                name: "displayName".to_string(),
                body: Some("return \"OrderService\";".to_string()),
                metadata: json!({"signature": "displayName(): string"}),
            },
        ),
    ];
    let graph = materialize(&records).unwrap();
    let first = render_files(&graph);
    let second = render_files(&graph);

    assert_eq!(first, second);
    assert!(first[0].content.contains("bonhomme:symbol="));
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

    validate_typescript_files(&rendered).await.unwrap();
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

    assert_eq!(graph.find_symbol("Inventory.ts").len(), 1);
    assert_eq!(graph.find_symbol("describeInventory").len(), 1);
    assert!(rendered[0].content.contains("describeInventory"));

    validate_typescript_files(&rendered).await.unwrap();
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

    validate_typescript_files(&rendered).await.unwrap();
}

fn import_graph(content: &str) -> SemanticGraph {
    let files = vec![RenderedFile {
        path: "src/Sample.ts".to_string(),
        content: content.to_string(),
    }];
    let operations = import_typescript_files(&files).unwrap();
    let records = operations
        .into_iter()
        .enumerate()
        .map(|(index, operation)| record(index as i64 + 1, operation))
        .collect::<Vec<_>>();
    materialize(&records).unwrap()
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
