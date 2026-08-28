use super::{
    KotlinPlugin, import_kotlin_files, read_kotlin_tree, recover_kotlin_operations, render_files,
    validate_kotlin_files,
};
use bonhomme_core::{
    Handler, Operation, OperationRecord, RenderedFile, SemanticGraph, SymbolNode, materialize,
};
use chrono::Utc;
use std::{fs, process::Command};
use uuid::Uuid;

#[test]
fn handler_claims_kotlin_sources() {
    let plugin = KotlinPlugin::default();

    assert!(plugin.claims(&path_only("src/OrderService.kt")));
    assert!(plugin.claims(&path_only("scripts/demo.kts")));
    assert!(!plugin.claims(&path_only("src/OrderService.java")));
}

#[test]
fn read_source_tree_includes_kotlin_sources() {
    let root = std::env::temp_dir().join(format!("bonhomme-kotlin-source-{}", Uuid::new_v4()));
    let src = root.join("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("OrderService.kt"), sample_source()).unwrap();
    fs::write(root.join("README.md"), "# demo\n").unwrap();

    let files = read_kotlin_tree(&root).unwrap();
    let paths = files.into_iter().map(|file| file.path).collect::<Vec<_>>();

    fs::remove_dir_all(&root).unwrap();

    assert_eq!(paths, vec!["src/OrderService.kt".to_string()]);
}

#[test]
fn import_renders_kotlin_symbols_and_calls() {
    let graph = import_graph(sample_source());

    let service = graph.find_symbol("OrderService")[0];
    let display_name = child_named(&graph, service.id, "displayName");
    let summary = child_named(&graph, service.id, "summary");
    let format_order = child_named(&graph, service.id, "formatOrder");
    let version = child_named(&graph, service.id, "version");
    let normalize = graph.find_symbol("normalizeOrderId")[0];
    let formatter = child_named(&graph, service.id, "Formatter");

    assert_eq!(service.kind, "class");
    assert_eq!(version.kind, "property");
    assert_eq!(formatter.kind, "object");
    assert_eq!(graph.find_symbol("NamesOrders")[0].kind, "interface");
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
    assert!(content.contains("package order"));
    assert!(content.contains("import kotlin.text.uppercase"));
    assert!(content.contains("kt-doc-marker"));
    assert!(content.contains("class OrderService"));
    assert!(content.contains("object Formatter"));
}

#[test]
fn imported_kotlin_round_trips_through_render_and_reparse() {
    let graph = import_graph(sample_source());
    let rendered = render_files(&graph);
    let reparsed = import_graph(&rendered[0].content);
    let rerendered = render_files(&reparsed);

    assert_eq!(rendered, rerendered);
}

#[tokio::test]
async fn imported_kotlin_compiles_when_kotlinc_is_available() {
    let graph = import_graph(sample_source());
    let rendered = render_files(&graph);

    if !kotlinc_available() {
        return;
    }

    validate_kotlin_files(&rendered).await.unwrap();
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
package order

class OrderService {
    fun displayName(orderId: String): String {
        return "order:$orderId"
    }

    fun summary(orderId: String): String {
        return formatOrder(displayName(orderId))
    }

    fun formatOrder(value: String): String {
        return value.uppercase()
    }
}
"#,
    )];

    let recovered = recover_kotlin_operations(&graph, &[summary_id], &edited).unwrap();

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
package order

import kotlin.text.uppercase

fun normalizeOrderId(orderId: String): String {
    return orderId.trim()
}

interface NamesOrders {
    fun displayName(orderId: String): String
}

class OrderService(private val prefix: String = "order") : NamesOrders {
    val version: String = "1"

    init {
        require(prefix.isNotBlank())
    }

    /** kt-doc-marker */
    override fun displayName(orderId: String): String {
        val normalized = normalizeOrderId(orderId)
        return "$prefix:$normalized"
    }

    fun summary(orderId: String): String {
        return formatOrder(displayName(orderId))
    }

    private fun formatOrder(value: String): String {
        return value.uppercase()
    }

    object Formatter {
        fun apply(value: String): String {
            return value
        }
    }
}
"#
}

fn recovery_base_source() -> &'static str {
    r#"
package order

class OrderService {
    fun displayName(orderId: String): String {
        return "order:$orderId"
    }

    fun summary(orderId: String): String {
        return displayName(orderId)
    }

    fun formatOrder(value: String): String {
        return value.uppercase()
    }
}
"#
}

fn import_graph(content: &str) -> SemanticGraph {
    materialize_operations(import_operations(content))
}

fn import_operations(content: &str) -> Vec<Operation> {
    import_kotlin_files(&[sample_file(content)]).unwrap()
}

fn sample_file(content: &str) -> RenderedFile {
    RenderedFile {
        path: "src/OrderService.kt".to_string(),
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

fn kotlinc_available() -> bool {
    Command::new("kotlinc")
        .arg("-version")
        .output()
        .is_ok_and(|output| output.status.success())
}
