use super::{
    SwiftPlugin, import_swift_files, read_swift_tree, recover_swift_operations, render_files,
    validate_swift_files,
};
use bonhomme_core::{
    Handler, Operation, OperationRecord, RenderedFile, SemanticGraph, SymbolNode, materialize,
};
use chrono::Utc;
use std::fs;
use uuid::Uuid;

#[test]
fn handler_claims_swift_sources() {
    let plugin = SwiftPlugin::default();

    assert!(plugin.claims(&path_only("Sources/OrderService.swift")));
    assert!(!plugin.claims(&path_only("src/OrderService.kt")));
    assert!(!plugin.claims(&path_only("README.md")));
}

#[test]
fn read_source_tree_includes_swift_sources() {
    let root = std::env::temp_dir().join(format!("bonhomme-swift-source-{}", Uuid::new_v4()));
    let src = root.join("Sources");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("OrderService.swift"), sample_source()).unwrap();
    fs::write(root.join("README.md"), "# demo\n").unwrap();

    let files = read_swift_tree(&root).unwrap();
    let paths = files.into_iter().map(|file| file.path).collect::<Vec<_>>();

    fs::remove_dir_all(&root).unwrap();

    assert_eq!(paths, vec!["Sources/OrderService.swift".to_string()]);
}

#[test]
fn import_renders_swift_symbols_and_calls() {
    let graph = import_graph(sample_source());

    let service = graph.find_symbol("OrderService")[0];
    let prefix = child_named(&graph, service.id, "prefix");
    let display_name = child_named(&graph, service.id, "displayName");
    let summary = child_named(&graph, service.id, "summary");
    let format_order = child_named(&graph, service.id, "formatOrder");
    let normalize = graph.find_symbol("normalizeOrderId")[0];
    let formatter = child_named(&graph, service.id, "Formatter");

    assert_eq!(service.kind, "class");
    assert_eq!(prefix.kind, "property");
    assert_eq!(formatter.kind, "struct");
    assert_eq!(graph.find_symbol("NamesOrders")[0].kind, "protocol");
    assert_eq!(graph.find_symbol("OrderStatus")[0].kind, "enum");
    assert_eq!(normalize.kind, "function");

    let summary_callees = graph.find_callees(summary.id, "calls");
    assert_eq!(summary_callees.len(), 2);
    assert!(
        summary_callees
            .iter()
            .any(|callee| callee.id == display_name.id)
    );
    assert!(
        summary_callees
            .iter()
            .any(|callee| callee.id == format_order.id)
    );

    let display_callees = graph.find_callees(display_name.id, "calls");
    assert!(
        display_callees
            .iter()
            .any(|callee| callee.id == normalize.id)
    );

    let rendered = render_files(&graph);
    let content = &rendered[0].content;
    assert!(content.contains("import Foundation"));
    assert!(content.contains("swift-doc-marker"));
    assert!(content.contains("final class OrderService"));
    assert!(content.contains("struct Formatter"));
}

#[test]
fn imported_swift_round_trips_through_render_and_reparse() {
    let graph = import_graph(sample_source());
    let rendered = render_files(&graph);
    let reparsed = import_graph(&rendered[0].content);
    let rerendered = render_files(&reparsed);

    assert_eq!(rendered, rerendered);
}

#[tokio::test]
async fn imported_swift_round_trips_and_parses_with_swiftc() {
    let graph = import_graph(sample_source());
    let rendered = render_files(&graph);

    validate_swift_files(&rendered).await.unwrap();
}

#[test]
fn recovery_updates_method_body_and_call_references() {
    let mut operations = import_operations(recovery_base_source());
    let graph = materialize_operations(operations.clone());
    let service = graph.find_symbol("OrderService")[0];
    let summary_id = child_named(&graph, service.id, "summary").id;
    let format_order_id = child_named(&graph, service.id, "formatOrder").id;
    let edited = vec![sample_file(
        r#"
final class OrderService {
    func displayName(_ orderId: String) -> String {
        return "order:\(orderId)"
    }

    func summary(_ orderId: String) -> String {
        return formatOrder(displayName(orderId))
    }

    func formatOrder(_ value: String) -> String {
        return value.uppercased()
    }
}
"#,
    )];

    let recovered = recover_swift_operations(&graph, &[summary_id], &edited).unwrap();

    assert!(recovered.iter().any(|operation| matches!(
        operation,
        Operation::UpdateSymbol { symbol_id, body: Some(body), .. }
            if *symbol_id == summary_id && body.contains("formatOrder")
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
import Foundation

func normalizeOrderId(_ orderId: String) -> String {
    return orderId.trimmingCharacters(in: .whitespacesAndNewlines)
}

protocol NamesOrders {
    func displayName(_ orderId: String) -> String
}

enum OrderStatus {
    case open
    case closed
}

final class OrderService: NamesOrders {
    let prefix: String

    init(prefix: String = "order") {
        self.prefix = prefix
    }

    /// swift-doc-marker
    func displayName(_ orderId: String) -> String {
        let normalized = normalizeOrderId(orderId)
        return "\(prefix):\(normalized)"
    }

    func summary(_ orderId: String) -> String {
        return formatOrder(displayName(orderId))
    }

    private func formatOrder(_ value: String) -> String {
        return value.uppercased()
    }

    struct Formatter {
        func apply(_ value: String) -> String {
            return value
        }
    }
}
"#
}

fn recovery_base_source() -> &'static str {
    r#"
final class OrderService {
    func displayName(_ orderId: String) -> String {
        return "order:\(orderId)"
    }

    func summary(_ orderId: String) -> String {
        return displayName(orderId)
    }

    func formatOrder(_ value: String) -> String {
        return value.uppercased()
    }
}
"#
}

fn import_graph(content: &str) -> SemanticGraph {
    materialize_operations(import_operations(content))
}

fn import_operations(content: &str) -> Vec<Operation> {
    import_swift_files(&[sample_file(content)]).unwrap()
}

fn sample_file(content: &str) -> RenderedFile {
    RenderedFile {
        path: "Sources/OrderService.swift".to_string(),
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

fn child_named<'a>(graph: &'a SemanticGraph, parent_id: Uuid, name: &str) -> &'a SymbolNode {
    graph
        .children_of(parent_id)
        .into_iter()
        .find(|symbol| symbol.name == name)
        .unwrap_or_else(|| panic!("expected child symbol named {name}"))
}
