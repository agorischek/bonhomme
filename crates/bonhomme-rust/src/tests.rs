use super::{import_rust_files, recover_rust_operations, render_files, validate_rust_files};
use bonhomme_core::{Operation, OperationRecord, RenderedFile, SemanticGraph, materialize};
use chrono::Utc;
use uuid::Uuid;

#[test]
fn import_renders_impl_methods_as_type_children() {
    let graph = import_graph(sample_source());

    let service = graph.find_symbol("OrderService")[0];
    let display_name = graph.find_symbol("display_name")[0];
    let summary = graph.find_symbol("summary")[0];
    let format_order = graph.find_symbol("format_order")[0];

    assert_eq!(display_name.parent_id, Some(service.id));
    assert_eq!(summary.parent_id, Some(service.id));

    let callees = graph.find_callees(summary.id, "calls");
    assert_eq!(callees.len(), 2);
    assert!(callees.iter().any(|callee| callee.id == display_name.id));
    assert!(callees.iter().any(|callee| callee.id == format_order.id));

    let rendered = render_files(&graph);
    assert!(rendered[0].content.contains("pub struct OrderService"));
    assert!(
        rendered[0]
            .content
            .contains("impl OrderService {\n    pub fn display_name")
    );
    assert!(rendered[0].content.contains("pub fn format_order"));
}

#[test]
fn type_level_attributes_round_trip() {
    // Regression: `#[derive]`/`#[serde]` on a struct or enum were dropped on import, so the render
    // came back missing them (which would not compile and would change the serde wire format).
    let source = "\
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = \"type\", rename_all = \"camelCase\")]
pub enum Shape {
    Circle { radius: f64 },
    Square { side: f64 },
}
";
    let content = render_files(&import_graph(source))[0].content.clone();

    assert!(
        content.contains("#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]"),
        "derive attribute dropped: {content}"
    );
    assert!(
        content.contains("#[serde("),
        "serde attribute dropped: {content}"
    );
    assert!(
        content.contains("tag = \"type\""),
        "serde args dropped: {content}"
    );
    assert!(
        content.contains("rename_all = \"camelCase\""),
        "serde args dropped: {content}"
    );
}

#[tokio::test]
async fn imported_rust_round_trips_and_builds() {
    let graph = import_graph(sample_source());
    let rendered = render_files(&graph);

    validate_rust_files(&rendered).await.unwrap();
    let reparsed = import_graph(&rendered[0].content);
    let rerendered = render_files(&reparsed);
    assert_eq!(rendered, rerendered);
}

#[tokio::test]
async fn validation_checks_non_root_rendered_files() {
    validate_rust_files(&[RenderedFile {
        path: "order/service.rs".to_string(),
        content: "pub fn answer() -> usize { 42 }\n".to_string(),
    }])
    .await
    .unwrap();

    let invalid = validate_rust_files(&[RenderedFile {
        path: "order/service.rs".to_string(),
        content: "pub fn broken() -> MissingType { 42 }\n".to_string(),
    }])
    .await;
    assert!(invalid.is_err());
}

#[test]
fn recovery_updates_method_body_and_call_references() {
    let mut operations = import_operations(recovery_base_source());
    let graph = materialize_operations(operations.clone());
    let summary_id = graph.find_symbol("summary")[0].id;
    let format_order_id = graph.find_symbol("format_order")[0].id;
    let edited = vec![sample_file(
        r#"
pub struct OrderService {
    service_name: String,
}

impl OrderService {
    pub fn display_name(&self) -> &str {
        &self.service_name
    }

    pub fn list_orders(&self) -> Vec<&'static str> {
        vec!["intake", "packing"]
    }

    pub fn summary(&self) -> String {
        format_order(self.display_name())
    }
}

pub fn format_order(id: &str) -> String {
    format!("order:{id}")
}
"#,
    )];

    let recovered = recover_rust_operations(&graph, &[summary_id], &edited).unwrap();

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
pub struct OrderService {
    service_name: String,
}

impl OrderService {
    pub fn display_name(&self) -> &str {
        &self.service_name
    }

    pub fn list_orders(&self) -> Vec<&'static str> {
        vec!["intake", "packing"]
    }

    pub fn summary(&self) -> String {
        format_order(self.display_name())
    }
}

pub fn format_order(id: &str) -> String {
    format!("order:{id}")
}
"#
}

fn recovery_base_source() -> &'static str {
    r#"
pub struct OrderService {
    service_name: String,
}

impl OrderService {
    pub fn display_name(&self) -> &str {
        &self.service_name
    }

    pub fn list_orders(&self) -> Vec<&'static str> {
        vec!["intake", "packing"]
    }

    pub fn summary(&self) -> String {
        self.display_name().to_string()
    }
}

pub fn format_order(id: &str) -> String {
    format!("order:{id}")
}
"#
}

fn import_graph(content: &str) -> SemanticGraph {
    materialize_operations(import_operations(content))
}

fn import_operations(content: &str) -> Vec<Operation> {
    import_rust_files(&[sample_file(content)]).unwrap()
}

fn sample_file(content: &str) -> RenderedFile {
    RenderedFile {
        path: "src/lib.rs".to_string(),
        content: content.to_string(),
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
