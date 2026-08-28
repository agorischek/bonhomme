use super::{
    JavaPlugin, import_java_files, read_java_tree, recover_java_operations, render_files,
    validate_java_files,
};
use bonhomme_core::{
    Handler, Operation, OperationRecord, RenderedFile, SemanticGraph, materialize,
};
use chrono::Utc;
use std::{fs, process::Command};
use uuid::Uuid;

#[test]
fn handler_claims_java_sources() {
    let plugin = JavaPlugin::default();

    assert!(plugin.claims(&path_only("src/main/java/com/acme/OrderService.java")));
    assert!(!plugin.claims(&path_only("src/service.py")));
    assert!(!plugin.claims(&path_only("README.md")));
}

#[test]
fn read_source_tree_includes_java_sources() {
    let root = std::env::temp_dir().join(format!("bonhomme-java-source-{}", Uuid::new_v4()));
    let src = root.join("src/main/java/com/acme");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("OrderService.java"), sample_source()).unwrap();
    fs::write(root.join("README.md"), "# demo\n").unwrap();

    let files = read_java_tree(&root).unwrap();
    let paths = files.into_iter().map(|file| file.path).collect::<Vec<_>>();

    fs::remove_dir_all(&root).unwrap();

    assert_eq!(
        paths,
        vec!["src/main/java/com/acme/OrderService.java".to_string()]
    );
}

#[test]
fn import_renders_java_types_members_nested_symbols_and_calls() {
    let graph = import_graph(sample_source());

    let service = graph
        .find_symbol("OrderService")
        .into_iter()
        .find(|symbol| symbol.kind == "class")
        .expect("OrderService class should be present");
    let prefix = graph.find_symbol("prefix")[0];
    let display_name = child_named(&graph, service.id, "displayName");
    let summary = child_named(&graph, service.id, "summary");
    let format_order = child_named(&graph, service.id, "formatOrder");
    let formatter = graph.find_symbol("Formatter")[0];
    let status = graph.find_symbol("Status")[0];
    let annotation_value = graph.find_symbol("value")[0];

    assert_eq!(service.kind, "class");
    assert_eq!(prefix.kind, "field");
    assert_eq!(prefix.parent_id, Some(service.id));
    assert_eq!(display_name.parent_id, Some(service.id));
    assert_eq!(formatter.kind, "class");
    assert_eq!(formatter.parent_id, Some(service.id));
    assert_eq!(status.kind, "enum");
    assert_eq!(status.parent_id, Some(service.id));
    assert_eq!(annotation_value.kind, "annotation-element");

    let callees = graph.find_callees(summary.id, "calls");
    assert_eq!(callees.len(), 2);
    assert!(callees.iter().any(|callee| callee.id == display_name.id));
    assert!(callees.iter().any(|callee| callee.id == format_order.id));

    let rendered = render_files(&graph);
    let content = &rendered[0].content;
    assert!(content.contains("package com.acme.orders;"));
    assert!(content.contains("import java.util.Locale;"));
    assert!(content.contains("java-doc-marker"));
    assert!(content.contains("public final class OrderService implements Named"));
    assert!(content.contains("private final String prefix = \"order\";"));
    assert!(content.contains("static final class Formatter"));
    assert!(content.contains("enum Status"));
    assert!(content.contains("OPEN,"));
    assert!(content.contains("CLOSED;"));
}

#[test]
fn overloaded_methods_get_distinct_stable_symbols() {
    let graph = import_graph(
        r#"
class Saves {
    void save(String value) {}
    void save(int value) {}
}
"#,
    );

    let saves = graph
        .symbols
        .values()
        .filter(|symbol| symbol.kind == "method" && symbol.name.starts_with("save("))
        .collect::<Vec<_>>();
    assert_eq!(saves.len(), 2);
    assert_ne!(saves[0].id, saves[1].id);
    assert!(
        saves
            .iter()
            .any(|symbol| symbol.name == "save(String value)")
    );
    assert!(saves.iter().any(|symbol| symbol.name == "save(int value)"));
}

#[test]
fn imported_java_round_trips_through_render_and_reparse() {
    let graph = import_graph(sample_source());
    let rendered = render_files(&graph);
    let reparsed = import_graph(&rendered[0].content);
    let rerendered = render_files(&reparsed);

    assert_eq!(rendered, rerendered);
}

#[tokio::test]
async fn imported_java_compiles_when_javac_is_available() {
    let graph = import_graph(sample_source());
    let rendered = render_files(&graph);

    if !javac_available() {
        return;
    }

    validate_java_files(&rendered).await.unwrap();
}

#[test]
fn recovery_updates_method_body_and_call_references() {
    let mut operations = import_operations(recovery_base_source());
    let graph = materialize_operations(operations.clone());
    let summary_id = graph.find_symbol("summary")[0].id;
    let format_order_id = graph.find_symbol("formatOrder")[0].id;
    let edited = vec![sample_file(
        r#"
package com.acme.orders;

final class OrderService {
    String displayName(String orderId) {
        return "order:" + orderId;
    }

    String summary(String orderId) {
        return formatOrder(displayName(orderId));
    }

    static String formatOrder(String value) {
        return value.toUpperCase();
    }
}
"#,
    )];

    let recovered = recover_java_operations(&graph, &[summary_id], &edited).unwrap();

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
package com.acme.orders;

import java.util.Locale;

@interface Audited {
    String value() default "";
}

interface Named {
    String displayName(String orderId);
}

record OrderId(String value) {
    OrderId {
        if (value.isBlank()) {
            throw new IllegalArgumentException("empty");
        }
    }
}

/** java-doc-marker */
public final class OrderService implements Named {
    private final String prefix = "order";

    public OrderService() {
    }

    @Override
    public String displayName(String orderId) {
        return prefix + ":" + orderId;
    }

    public String summary(String orderId) {
        return formatOrder(displayName(orderId));
    }

    private static String formatOrder(String value) {
        return value.toUpperCase(Locale.ROOT);
    }

    static final class Formatter {
        String apply(String value) {
            return formatOrder(value);
        }
    }

    enum Status {
        OPEN,
        CLOSED;

        boolean done() {
            return this == CLOSED;
        }
    }
}
"#
}

fn recovery_base_source() -> &'static str {
    r#"
package com.acme.orders;

final class OrderService {
    String displayName(String orderId) {
        return "order:" + orderId;
    }

    String summary(String orderId) {
        return displayName(orderId);
    }

    static String formatOrder(String value) {
        return value.toUpperCase();
    }
}
"#
}

fn import_graph(content: &str) -> SemanticGraph {
    materialize_operations(import_operations(content))
}

fn import_operations(content: &str) -> Vec<Operation> {
    import_java_files(&[sample_file(content)]).unwrap()
}

fn sample_file(content: &str) -> RenderedFile {
    RenderedFile {
        path: "src/main/java/com/acme/orders/OrderService.java".to_string(),
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

fn child_named<'a>(
    graph: &'a SemanticGraph,
    parent_id: Uuid,
    name: &str,
) -> &'a bonhomme_core::SymbolNode {
    graph
        .children_of(parent_id)
        .into_iter()
        .find(|symbol| symbol.name == name)
        .unwrap_or_else(|| panic!("expected child symbol named {name}"))
}

fn javac_available() -> bool {
    Command::new("javac")
        .arg("-version")
        .output()
        .is_ok_and(|output| output.status.success())
}
