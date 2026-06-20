use super::{
    ElixirPlugin, import_elixir_files, read_elixir_tree, recover_elixir_operations, render_files,
    validate_elixir_files,
};
use bonhomme_core::{
    Handler, Operation, OperationRecord, RenderedFile, SemanticGraph, materialize,
};
use chrono::Utc;
use std::fs;
use uuid::Uuid;

#[test]
fn handler_claims_elixir_sources() {
    let plugin = ElixirPlugin::default();

    assert!(plugin.claims(&path_only("lib/service.ex")));
    assert!(plugin.claims(&path_only("script/report.exs")));
    assert!(!plugin.claims(&path_only("src/service.py")));
    assert!(!plugin.claims(&path_only("README.md")));
}

#[test]
fn read_source_tree_includes_elixir_sources() {
    let root = std::env::temp_dir().join(format!("bonhomme-elixir-source-{}", Uuid::new_v4()));
    let lib = root.join("lib");
    fs::create_dir_all(&lib).unwrap();
    fs::write(lib.join("service.ex"), sample_source()).unwrap();
    fs::write(root.join("script.exs"), "IO.puts(:ok)\n").unwrap();
    fs::write(root.join("README.md"), "# demo\n").unwrap();

    let files = read_elixir_tree(&root).unwrap();
    let paths = files.into_iter().map(|file| file.path).collect::<Vec<_>>();

    fs::remove_dir_all(&root).unwrap();

    assert_eq!(
        paths,
        vec!["lib/service.ex".to_string(), "script.exs".to_string()]
    );
}

#[test]
fn import_renders_modules_functions_clauses_and_calls() {
    let graph = import_graph(sample_source());

    let service = graph.find_symbol("Bonhomme.Example.OrderService")[0];
    let display_name = graph.find_symbol("display_name/1")[0];
    let summary = graph.find_symbol("summary/1")[0];
    let format_order = graph.find_symbol("format_order/1")[0];
    let status = graph.find_symbol("status/1")[0];

    assert_eq!(service.kind, "module");
    assert_eq!(display_name.kind, "function");
    assert_eq!(display_name.parent_id, Some(service.id));
    assert_eq!(summary.parent_id, Some(service.id));
    assert_eq!(status.parent_id, Some(service.id));
    assert!(
        status
            .body
            .as_deref()
            .unwrap()
            .contains("def status(:closed)")
    );

    let callees = graph.find_callees(summary.id, "calls");
    assert_eq!(callees.len(), 2);
    assert!(callees.iter().any(|callee| callee.id == display_name.id));
    assert!(callees.iter().any(|callee| callee.id == format_order.id));

    let rendered = render_files(&graph);
    assert!(rendered[0].content.contains("@prefix \"order\""));
    assert!(
        rendered[0]
            .content
            .contains("defmodule Bonhomme.Example.OrderService do")
    );
    assert!(
        rendered[0]
            .content
            .contains("def display_name(order_id) do")
    );
    assert!(rendered[0].content.contains("def status(:open)"));
}

#[tokio::test]
async fn imported_elixir_round_trips_and_compiles() {
    let graph = import_graph(sample_source());
    let rendered = render_files(&graph);

    validate_elixir_files(&rendered).await.unwrap();
    let reparsed = import_graph(&rendered[0].content);
    let rerendered = render_files(&reparsed);
    assert_eq!(rendered, rerendered);
}

#[test]
fn recovery_updates_function_body_and_call_references() {
    let mut operations = import_operations(recovery_base_source());
    let graph = materialize_operations(operations.clone());
    let summary_id = graph.find_symbol("summary/1")[0].id;
    let format_order_id = graph.find_symbol("format_order/1")[0].id;
    let edited = vec![sample_file(sample_source())];

    let recovered = recover_elixir_operations(&graph, &[summary_id], &edited).unwrap();

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
    r##"
defmodule Bonhomme.Example.OrderService do
  @prefix "order"

  def display_name(order_id) do
    "#{@prefix}:#{order_id}"
  end

  def summary(order_id) do
    order_id
    |> display_name()
    |> format_order()
  end

  def format_order(value) do
    String.upcase(value)
  end

  def status(:open), do: :active
  def status(:closed), do: :done
end
"##
}

fn recovery_base_source() -> &'static str {
    r##"
defmodule Bonhomme.Example.OrderService do
  @prefix "order"

  def display_name(order_id) do
    "#{@prefix}:#{order_id}"
  end

  def summary(order_id) do
    display_name(order_id)
  end

  def format_order(value) do
    String.upcase(value)
  end
end
"##
}

fn import_graph(content: &str) -> SemanticGraph {
    materialize_operations(import_operations(content))
}

fn import_operations(content: &str) -> Vec<Operation> {
    import_elixir_files(&[sample_file(content)]).unwrap()
}

fn sample_file(content: &str) -> RenderedFile {
    RenderedFile {
        path: "lib/order_service.ex".to_string(),
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
