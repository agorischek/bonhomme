use super::{
    PythonPlugin, import_python_files, read_python_tree, recover_python_operations, render_files,
    validate_python_files,
};
use bonhomme_core::{
    Handler, Operation, OperationRecord, RenderedFile, SemanticGraph, materialize,
};
use chrono::Utc;
use std::fs;
use uuid::Uuid;

#[test]
fn handler_claims_python_sources() {
    let plugin = PythonPlugin::default();

    assert!(plugin.claims(&path_only("src/service.py")));
    assert!(plugin.claims(&path_only("src/service.pyi")));
    assert!(!plugin.claims(&path_only("src/service.rs")));
    assert!(!plugin.claims(&path_only("README.md")));
}

#[test]
fn read_source_tree_includes_python_sources() {
    let root = std::env::temp_dir().join(format!("bonhomme-python-source-{}", Uuid::new_v4()));
    let src = root.join("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("service.py"), "def answer():\n    return 42\n").unwrap();
    fs::write(src.join("service.pyi"), "def answer() -> int: ...\n").unwrap();
    fs::write(root.join("README.md"), "# demo\n").unwrap();

    let files = read_python_tree(&root).unwrap();
    let paths = files.into_iter().map(|file| file.path).collect::<Vec<_>>();

    fs::remove_dir_all(&root).unwrap();

    assert_eq!(
        paths,
        vec!["src/service.py".to_string(), "src/service.pyi".to_string()]
    );
}

#[test]
fn import_renders_classes_methods_values_and_calls() {
    let graph = import_graph(sample_source());

    let service = graph.find_symbol("OrderService")[0];
    let prefix = graph.find_symbol("prefix")[0];
    let display_name = graph.find_symbol("display_name")[0];
    let summary = graph.find_symbol("summary")[0];
    let format_order = graph.find_symbol("format_order")[0];

    assert_eq!(service.kind, "class");
    assert_eq!(prefix.parent_id, Some(service.id));
    assert_eq!(display_name.parent_id, Some(service.id));
    assert_eq!(summary.parent_id, Some(service.id));

    let callees = graph.find_callees(summary.id, "calls");
    assert_eq!(callees.len(), 2);
    assert!(callees.iter().any(|callee| callee.id == display_name.id));
    assert!(callees.iter().any(|callee| callee.id == format_order.id));

    let rendered = render_files(&graph);
    assert!(
        rendered[0]
            .content
            .contains("from __future__ import annotations")
    );
    assert!(rendered[0].content.contains("MODULE_TIMEOUT = 30"));
    assert!(rendered[0].content.contains("class OrderService:"));
    assert!(rendered[0].content.contains("prefix = \"order\""));
    assert!(rendered[0].content.contains("def format_order"));
}

#[tokio::test]
async fn imported_python_round_trips_and_compiles() {
    let graph = import_graph(sample_source());
    let rendered = render_files(&graph);

    validate_python_files(&rendered).await.unwrap();
    let reparsed = import_graph(&rendered[0].content);
    let rerendered = render_files(&reparsed);
    assert_eq!(rendered, rerendered);
}

#[test]
fn recovery_updates_method_body_and_call_references() {
    let mut operations = import_operations(recovery_base_source());
    let graph = materialize_operations(operations.clone());
    let summary_id = graph.find_symbol("summary")[0].id;
    let format_order_id = graph.find_symbol("format_order")[0].id;
    let edited = vec![sample_file(
        r#"
class OrderService:
    prefix = "order"

    def display_name(self, order_id: str) -> str:
        return f"{self.prefix}:{order_id}"

    def list_orders(self) -> list[str]:
        return ["intake", "packing"]

    def summary(self, order_id: str) -> str:
        return format_order(self.display_name(order_id))


def format_order(value: str) -> str:
    return value.upper()
"#,
    )];

    let recovered = recover_python_operations(&graph, &[summary_id], &edited).unwrap();

    assert!(recovered.iter().any(|operation| matches!(
        operation,
        Operation::UpdateSymbol { symbol_id, body: Some(body), .. }
            if *symbol_id == summary_id && body.contains("format_order")
    )));
    assert!(recovered.iter().any(|operation| matches!(
        operation,
        Operation::CreateReference { from_symbol_id, to_symbol_id, kind, .. }
            if *from_symbol_id == summary_id
                && *to_symbol_id == format_order_id
                && kind == "calls"
    )));

    operations.extend(recovered);
    let updated = materialize_operations(operations);
    let callees = updated.find_callees(summary_id, "calls");
    assert!(callees.iter().any(|callee| callee.id == format_order_id));
}

fn sample_source() -> &'static str {
    r#"
from __future__ import annotations

MODULE_TIMEOUT = 30


class OrderService:
    """Coordinates order display."""

    prefix = "order"

    def display_name(self, order_id: str) -> str:
        return f"{self.prefix}:{order_id}"

    def list_orders(self) -> list[str]:
        return ["intake", "packing"]

    def summary(self, order_id: str) -> str:
        return format_order(self.display_name(order_id))


def format_order(value: str) -> str:
    return value.upper()
"#
}

fn recovery_base_source() -> &'static str {
    r#"
class OrderService:
    prefix = "order"

    def display_name(self, order_id: str) -> str:
        return f"{self.prefix}:{order_id}"

    def list_orders(self) -> list[str]:
        return ["intake", "packing"]

    def summary(self, order_id: str) -> str:
        return self.display_name(order_id)


def format_order(value: str) -> str:
    return value.upper()
"#
}

fn import_graph(content: &str) -> SemanticGraph {
    materialize_operations(import_operations(content))
}

fn import_operations(content: &str) -> Vec<Operation> {
    import_python_files(&[sample_file(content)]).unwrap()
}

fn sample_file(content: &str) -> RenderedFile {
    RenderedFile {
        path: "src/service.py".to_string(),
        content: content.to_string(),
    }
}

fn path_only(path: &str) -> RenderedFile {
    RenderedFile {
        path: path.to_string(),
        content: String::new(),
    }
}

fn materialize_operations(operations: Vec<Operation>) -> SemanticGraph {
    let records = operations
        .into_iter()
        .enumerate()
        .map(|(index, operation)| OperationRecord {
            id: Uuid::new_v4(),
            repository_id: Uuid::new_v4(),
            branch_id: Uuid::new_v4(),
            changeset_id: Uuid::new_v4(),
            position: index as i64 + 1,
            operation,
            created_at: Utc::now(),
        })
        .collect::<Vec<_>>();
    materialize(&records).unwrap()
}
