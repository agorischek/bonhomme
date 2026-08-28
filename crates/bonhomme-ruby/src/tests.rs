use super::{
    RubyPlugin, import_ruby_files, read_ruby_tree, recover_ruby_operations, render_files,
    validate_ruby_files,
};
use bonhomme_core::{
    Handler, Operation, OperationRecord, RenderedFile, SemanticGraph, SymbolNode, materialize,
};
use chrono::Utc;
use std::fs;
use uuid::Uuid;

#[test]
fn handler_claims_ruby_sources() {
    let plugin = RubyPlugin::default();

    assert!(plugin.claims(&path_only("lib/order_service.rb")));
    assert!(!plugin.claims(&path_only("lib/order_service.php")));
    assert!(!plugin.claims(&path_only("README.md")));
}

#[test]
fn read_source_tree_includes_ruby_sources() {
    let root = std::env::temp_dir().join(format!("bonhomme-ruby-source-{}", Uuid::new_v4()));
    let lib = root.join("lib");
    fs::create_dir_all(&lib).unwrap();
    fs::write(lib.join("order_service.rb"), sample_source()).unwrap();
    fs::write(root.join("README.md"), "# demo\n").unwrap();

    let files = read_ruby_tree(&root).unwrap();
    let paths = files.into_iter().map(|file| file.path).collect::<Vec<_>>();

    fs::remove_dir_all(&root).unwrap();

    assert_eq!(paths, vec!["lib/order_service.rb".to_string()]);
}

#[test]
fn import_renders_ruby_symbols_and_calls() {
    let graph = import_graph(sample_source());

    let order = graph.find_symbol("Order")[0];
    let service = child_named(&graph, order.id, "OrderService");
    let prefix = child_named(&graph, service.id, "PREFIX");
    let display_name = child_named(&graph, service.id, "display_name");
    let summary = child_named(&graph, service.id, "summary");
    let format_order = child_named(&graph, service.id, "format_order");
    let normalize = child_named(&graph, order.id, "normalize_order_id");
    let formatter = child_named(&graph, service.id, "Formatter");

    assert_eq!(order.kind, "module");
    assert_eq!(service.kind, "class");
    assert_eq!(prefix.kind, "constant");
    assert_eq!(normalize.kind, "singleton-method");
    assert_eq!(formatter.kind, "class");

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
    assert!(content.contains("# frozen_string_literal: true"));
    assert!(content.contains("ruby-doc-marker"));
    assert!(content.contains("module Order"));
    assert!(content.contains("class OrderService"));
    assert!(content.contains("PREFIX = \"order\""));
    assert!(content.contains("class Formatter"));
}

#[test]
fn imported_ruby_round_trips_through_render_and_reparse() {
    let graph = import_graph(sample_source());
    let rendered = render_files(&graph);
    let reparsed = import_graph(&rendered[0].content);
    let rerendered = render_files(&reparsed);

    assert_eq!(rendered, rerendered);
}

#[tokio::test]
async fn imported_ruby_round_trips_and_lints() {
    let graph = import_graph(sample_source());
    let rendered = render_files(&graph);

    validate_ruby_files(&rendered).await.unwrap();
}

#[test]
fn recovery_updates_method_body_and_call_references() {
    let mut operations = import_operations(recovery_base_source());
    let graph = materialize_operations(operations.clone());
    let service = graph.find_symbol("OrderService")[0];
    let summary_id = child_named(&graph, service.id, "summary").id;
    let format_order_id = child_named(&graph, service.id, "format_order").id;
    let edited = vec![sample_file(
        r##"
module Order
  class OrderService
    def display_name(order_id)
      "order:#{order_id}"
    end

    def summary(order_id)
      format_order(display_name(order_id))
    end

    def format_order(value)
      value.upcase
    end
  end
end
"##,
    )];

    let recovered = recover_ruby_operations(&graph, &[summary_id], &edited).unwrap();

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
# frozen_string_literal: true

module Order
  def self.normalize_order_id(order_id)
    order_id.strip
  end

  class OrderService
    PREFIX = "order"

    attr_reader :prefix

    def initialize(prefix = PREFIX)
      @prefix = prefix
    end

    # ruby-doc-marker
    def display_name(order_id)
      "#{@prefix}:#{Order.normalize_order_id(order_id)}"
    end

    def summary(order_id)
      format_order(display_name(order_id))
    end

    def format_order(value)
      value.upcase
    end

    class Formatter
      def apply(value)
        value.to_s
      end
    end
  end
end
"##
}

fn recovery_base_source() -> &'static str {
    r##"
module Order
  class OrderService
    def display_name(order_id)
      "order:#{order_id}"
    end

    def summary(order_id)
      display_name(order_id)
    end

    def format_order(value)
      value.upcase
    end
  end
end
"##
}

fn import_graph(content: &str) -> SemanticGraph {
    materialize_operations(import_operations(content))
}

fn import_operations(content: &str) -> Vec<Operation> {
    import_ruby_files(&[sample_file(content)]).unwrap()
}

fn sample_file(content: &str) -> RenderedFile {
    RenderedFile {
        path: "lib/order_service.rb".to_string(),
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
