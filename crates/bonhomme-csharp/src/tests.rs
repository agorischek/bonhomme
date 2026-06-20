use super::{
    CSharpPlugin, import_csharp_files, read_csharp_tree, recover_csharp_operations, render_files,
    validate_csharp_files,
};
use bonhomme_core::{
    Handler, Operation, OperationRecord, RenderedFile, SemanticGraph, materialize,
};
use chrono::Utc;
use std::fs;
use uuid::Uuid;

#[test]
fn handler_claims_csharp_sources() {
    let plugin = CSharpPlugin::default();

    assert!(plugin.claims(&path_only("src/OrderService.cs")));
    assert!(!plugin.claims(&path_only("src/service.py")));
    assert!(!plugin.claims(&path_only("README.md")));
}

#[test]
fn read_source_tree_includes_csharp_sources() {
    let root = std::env::temp_dir().join(format!("bonhomme-csharp-source-{}", Uuid::new_v4()));
    let src = root.join("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("OrderService.cs"), sample_source()).unwrap();
    fs::write(root.join("README.md"), "# demo\n").unwrap();

    let files = read_csharp_tree(&root).unwrap();
    let paths = files.into_iter().map(|file| file.path).collect::<Vec<_>>();

    fs::remove_dir_all(&root).unwrap();

    assert_eq!(paths, vec!["src/OrderService.cs".to_string()]);
}

#[test]
fn import_renders_types_members_and_calls() {
    let graph = import_graph(sample_source());

    let service = graph.find_symbol("OrderService")[0];
    let prefix = graph.find_symbol("_prefix")[0];
    let display_name = graph.find_symbol("DisplayName")[0];
    let summary = graph.find_symbol("Summary")[0];
    let format_order = graph.find_symbol("FormatOrder")[0];

    assert_eq!(service.kind, "class");
    assert_eq!(prefix.parent_id, Some(service.id));
    assert_eq!(display_name.parent_id, Some(service.id));
    assert_eq!(summary.parent_id, Some(service.id));

    let callees = graph.find_callees(summary.id, "calls");
    assert_eq!(callees.len(), 2);
    assert!(callees.iter().any(|callee| callee.id == display_name.id));
    assert!(callees.iter().any(|callee| callee.id == format_order.id));

    let rendered = render_files(&graph);
    assert!(rendered[0].content.contains("using System;"));
    assert!(rendered[0].content.contains("namespace Bonhomme.Example"));
    assert!(
        rendered[0]
            .content
            .contains("public sealed class OrderService")
    );
    assert!(
        rendered[0]
            .content
            .contains("private readonly string _prefix = \"order\";")
    );
    assert!(
        rendered[0]
            .content
            .contains("private static string FormatOrder")
    );
}

#[tokio::test]
async fn imported_csharp_round_trips_and_builds() {
    let graph = import_graph(sample_source());
    let rendered = render_files(&graph);

    validate_csharp_files(&rendered).await.unwrap();
    let reparsed = import_graph(&rendered[0].content);
    let rerendered = render_files(&reparsed);
    assert_eq!(rendered, rerendered);
}

#[test]
fn recovery_updates_method_body_and_call_references() {
    let mut operations = import_operations(recovery_base_source());
    let graph = materialize_operations(operations.clone());
    let summary_id = graph.find_symbol("Summary")[0].id;
    let format_order_id = graph.find_symbol("FormatOrder")[0].id;
    let edited = vec![sample_file(
        r#"
namespace Bonhomme.Example
{
    public sealed class OrderService
    {
        private readonly string _prefix = "order";

        public string DisplayName(string orderId)
        {
            return $"{_prefix}:{orderId}";
        }

        public string Summary(string orderId)
        {
            return FormatOrder(DisplayName(orderId));
        }

        private static string FormatOrder(string value)
        {
            return value.ToUpperInvariant();
        }
    }
}
"#,
    )];

    let recovered = recover_csharp_operations(&graph, &[summary_id], &edited).unwrap();

    assert!(recovered.iter().any(|operation| matches!(
        operation,
        Operation::UpdateSymbol { symbol_id, body: Some(body), .. }
            if *symbol_id == summary_id && body.contains("FormatOrder")
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
using System;

namespace Bonhomme.Example
{
    public sealed class OrderService
    {
        private readonly string _prefix = "order";

        public string DisplayName(string orderId)
        {
            return $"{_prefix}:{orderId}";
        }

        public string Summary(string orderId)
        {
            return FormatOrder(DisplayName(orderId));
        }

        private static string FormatOrder(string value)
        {
            return value.ToUpperInvariant();
        }
    }
}
"#
}

fn recovery_base_source() -> &'static str {
    r#"
namespace Bonhomme.Example
{
    public sealed class OrderService
    {
        private readonly string _prefix = "order";

        public string DisplayName(string orderId)
        {
            return $"{_prefix}:{orderId}";
        }

        public string Summary(string orderId)
        {
            return DisplayName(orderId);
        }

        private static string FormatOrder(string value)
        {
            return value.ToUpperInvariant();
        }
    }
}
"#
}

fn import_graph(content: &str) -> SemanticGraph {
    materialize_operations(import_operations(content))
}

fn import_operations(content: &str) -> Vec<Operation> {
    import_csharp_files(&[sample_file(content)]).unwrap()
}

fn sample_file(content: &str) -> RenderedFile {
    RenderedFile {
        path: "src/OrderService.cs".to_string(),
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
