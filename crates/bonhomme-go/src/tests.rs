use super::{import_go_files, recover_go_operations, render_files, validate_go_files};
use bonhomme_core::{Operation, OperationRecord, RenderedFile, SemanticGraph, materialize};
use chrono::Utc;
use uuid::Uuid;

#[test]
fn import_renders_receiver_methods_as_type_children() {
    let graph = import_graph(sample_source());

    let service = graph.find_symbol("OrderService")[0];
    let display_name = graph.find_symbol("DisplayName")[0];
    let summary = graph.find_symbol("Summary")[0];

    assert_eq!(display_name.parent_id, Some(service.id));
    assert_eq!(summary.parent_id, Some(service.id));
    assert_eq!(
        graph.find_callees(summary.id, "calls")[0].id,
        display_name.id
    );

    let rendered = render_files(&graph);
    assert!(rendered[0].content.contains("type OrderService struct"));
    assert!(
        rendered[0]
            .content
            .contains("func (s *OrderService) DisplayName() string")
    );
    assert!(
        rendered[0]
            .content
            .contains("func (s *OrderService) Summary() string")
    );
}

#[tokio::test]
async fn imported_go_round_trips_and_builds() {
    let graph = import_graph(sample_source());
    let rendered = render_files(&graph);

    validate_go_files(&rendered).await.unwrap();
    let reparsed = import_graph(&rendered[0].content);
    let rerendered = render_files(&reparsed);
    assert_eq!(rendered, rerendered);
}

#[test]
fn recovery_updates_method_body_and_call_references() {
    let mut operations = import_operations(sample_source());
    let graph = materialize_operations(operations.clone());
    let summary_id = graph.find_symbol("Summary")[0].id;
    let list_orders_id = graph.find_symbol("ListOrders")[0].id;
    let edited = vec![sample_file(
        r#"
package order

type OrderService struct {
	ServiceName string
}

func (s *OrderService) DisplayName() string {
	return s.ServiceName
}

func (s *OrderService) ListOrders() []string {
	return []string{"intake", "packing"}
}

func (s *OrderService) Summary() string {
	return s.ListOrders()[0]
}
"#,
    )];

    let recovered = recover_go_operations(&graph, &[summary_id], &edited).unwrap();

    assert!(matches!(
        recovered.as_slice(),
        [
            Operation::DeleteReference { .. },
            Operation::UpdateSymbol { symbol_id, .. },
            Operation::CreateReference {
                from_symbol_id,
                to_symbol_id,
                kind,
                ..
            }
        ] if *symbol_id == summary_id
            && *from_symbol_id == summary_id
            && *to_symbol_id == list_orders_id
            && kind == "calls"
    ));

    operations.extend(recovered);
    let updated = materialize_operations(operations);
    let callees = updated.find_callees(summary_id, "calls");
    assert_eq!(callees.len(), 1);
    assert_eq!(callees[0].id, list_orders_id);
}

fn sample_source() -> &'static str {
    r#"
package order

type OrderService struct {
	ServiceName string
}

func (s *OrderService) DisplayName() string {
	return s.ServiceName
}

func (s *OrderService) ListOrders() []string {
	return []string{"intake", "packing"}
}

func (s *OrderService) Summary() string {
	return s.DisplayName()
}
"#
}

fn import_graph(content: &str) -> SemanticGraph {
    materialize_operations(import_operations(content))
}

fn import_operations(content: &str) -> Vec<Operation> {
    import_go_files(&[sample_file(content)]).unwrap()
}

fn sample_file(content: &str) -> RenderedFile {
    RenderedFile {
        path: "order/service.go".to_string(),
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
