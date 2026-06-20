use super::record;
use crate::{render_files, render_slice};
use bonhomme_core::{Operation, materialize};
use serde_json::json;
use uuid::Uuid;

#[test]
fn files_are_deterministic_with_identity_comments() {
    let graph = order_service_graph();
    let first = render_files(&graph);
    let second = render_files(&graph);

    assert_eq!(first, second);
    assert!(first[0].content.contains("bonhomme:symbol="));
}

#[test]
fn slices_omit_identity_comments() {
    let graph = order_service_graph();
    let class = graph.find_symbol("OrderService")[0].id;

    let slice = render_slice(&graph, "main@3".to_string(), vec![class]);

    assert_eq!(slice.files.len(), 1);
    assert!(slice.files[0].content.contains("class OrderService"));
    assert!(!slice.files[0].content.contains("bonhomme:file="));
    assert!(!slice.files[0].content.contains("bonhomme:symbol="));
}

fn order_service_graph() -> bonhomme_core::SemanticGraph {
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
    materialize(&records).unwrap()
}
