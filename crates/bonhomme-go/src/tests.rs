use super::{
    import_go_files, recover_go_operations, render_files, validate_go_files,
    validate_go_files_with_workspace,
};
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

#[tokio::test]
async fn workspace_validation_uses_go_mod_and_embedded_assets() {
    let files = vec![
        RenderedFile {
            path: "go.mod".to_string(),
            content: "module example.com/bonhomme/workspace\n\ngo 1.22\n".to_string(),
        },
        RenderedFile {
            path: "cmd/app/main.go".to_string(),
            content: "package main\n\nimport _ \"example.com/bonhomme/workspace/internal/assets\"\n\nfunc main() {}\n".to_string(),
        },
        RenderedFile {
            path: "internal/assets/assets.go".to_string(),
            content: "package assets\n\nimport _ \"embed\"\n\n//go:embed data.txt\nvar Data string\n".to_string(),
        },
        RenderedFile {
            path: "internal/assets/data.txt".to_string(),
            content: "hello\n".to_string(),
        },
    ];
    let go_files = files
        .iter()
        .filter(|file| file.path.ends_with(".go"))
        .cloned()
        .collect::<Vec<_>>();

    validate_go_files_with_workspace(&files, &go_files)
        .await
        .unwrap();
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

#[test]
fn import_scopes_duplicate_function_names_by_package_directory() {
    let files = vec![
        RenderedFile {
            path: "cmd/alpha/main.go".to_string(),
            content: "package main\n\nfunc main() {}\n".to_string(),
        },
        RenderedFile {
            path: "cmd/beta/main.go".to_string(),
            content: "package main\n\nfunc main() {}\n".to_string(),
        },
    ];

    let graph = materialize_operations(import_go_files(&files).unwrap());
    let mains = graph
        .find_symbol("main")
        .into_iter()
        .filter(|symbol| symbol.kind == "function")
        .collect::<Vec<_>>();

    assert_eq!(mains.len(), 2);
    assert_ne!(mains[0].id, mains[1].id);
}

#[test]
fn import_scopes_build_variant_methods_by_file_path() {
    let files = vec![
        RenderedFile {
            path: "pkg/console/progress.go".to_string(),
            content: "package console\n\ntype ProgressBar struct{}\n\nfunc (p *ProgressBar) Update(current int64) string { return \"\" }\n".to_string(),
        },
        RenderedFile {
            path: "pkg/console/progress_wasm.go".to_string(),
            content: "package console\n\ntype ProgressBar struct{}\n\nfunc (p *ProgressBar) Update(current int64) string { return \"\" }\n".to_string(),
        },
    ];

    let graph = materialize_operations(import_go_files(&files).unwrap());
    let methods = graph
        .find_symbol("Update")
        .into_iter()
        .filter(|symbol| symbol.kind == "method")
        .collect::<Vec<_>>();

    assert_eq!(methods.len(), 2);
    assert_ne!(methods[0].id, methods[1].id);
    assert_ne!(methods[0].parent_id, methods[1].parent_id);
}

#[test]
fn import_handles_file_with_no_declarations() {
    let graph = materialize_operations(
        import_go_files(&[RenderedFile {
            path: "pkg/cli/doc.go".to_string(),
            content: "package cli\n".to_string(),
        }])
        .unwrap(),
    );

    assert_eq!(graph.root_symbols().len(), 1);
    assert_eq!(graph.root_symbols()[0].name, "pkg/cli/doc.go");
}

#[test]
fn import_allows_build_variant_methods_on_shared_receiver() {
    let files = vec![
        RenderedFile {
            path: "pkg/workflow/compiler_types.go".to_string(),
            content: "package workflow\n\ntype Compiler struct{}\n".to_string(),
        },
        RenderedFile {
            path: "pkg/workflow/dependabot.go".to_string(),
            content: "package workflow\n\nfunc (c *Compiler) GenerateDependabotManifests() error { return nil }\n".to_string(),
        },
        RenderedFile {
            path: "pkg/workflow/dependabot_wasm.go".to_string(),
            content: "//go:build wasm\n\npackage workflow\n\nfunc (c *Compiler) GenerateDependabotManifests() error { return nil }\n".to_string(),
        },
    ];

    let graph = materialize_operations(import_go_files(&files).unwrap());
    let methods = graph
        .find_symbol("GenerateDependabotManifests")
        .into_iter()
        .filter(|symbol| symbol.kind == "method")
        .collect::<Vec<_>>();
    let rendered = render_files(&graph);

    assert_eq!(methods.len(), 2);
    assert_ne!(methods[0].id, methods[1].id);
    assert_ne!(methods[0].parent_id, methods[1].parent_id);
    assert!(
        rendered
            .iter()
            .find(|file| file.path == "pkg/workflow/dependabot_wasm.go")
            .unwrap()
            .content
            .contains("func (c *Compiler) GenerateDependabotManifests() error")
    );
}

#[tokio::test]
async fn named_non_struct_types_render_their_methods() {
    let graph = import_graph(
        "package engine\n\n\
type configPresence map[string]bool\n\n\
func (p configPresence) has(path string) bool {\n\
\treturn p[path]\n\
}\n",
    );
    let rendered = render_files(&graph);
    let content = &rendered[0].content;

    assert!(content.contains("type configPresence map[string]bool"));
    assert!(content.contains("func (p configPresence) has(path string) bool"));
    validate_go_files(&rendered).await.unwrap();
}

#[tokio::test]
async fn methods_render_in_their_original_file_to_keep_imports_valid() {
    let files = vec![
        RenderedFile {
            path: "types.go".to_string(),
            content: "package engine\n\ntype Result struct{}\n".to_string(),
        },
        RenderedFile {
            path: "json_output.go".to_string(),
            content: "package engine\n\nimport \"encoding/json\"\n\nfunc (result Result) MarshalJSON() ([]byte, error) {\n\treturn json.Marshal(struct{}{})\n}\n".to_string(),
        },
    ];
    let graph = materialize_operations(import_go_files(&files).unwrap());
    let rendered = render_files(&graph);
    let by_path = rendered
        .iter()
        .map(|file| (file.path.as_str(), file.content.as_str()))
        .collect::<std::collections::BTreeMap<_, _>>();

    assert!(!by_path["types.go"].contains("MarshalJSON"));
    assert!(by_path["json_output.go"].contains("import \"encoding/json\""));
    assert!(by_path["json_output.go"].contains("func (result Result) MarshalJSON()"));
    validate_go_files_with_workspace(&files, &rendered)
        .await
        .unwrap();
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

#[test]
fn godoc_comments_round_trip() {
    // Regression: leading `// …` doc comments above Go declarations were dropped on render.
    let source = "package order\n\n\
// Greeter greets people by name.\n\
type Greeter struct {\n\tName string\n}\n\n\
// Hello returns a greeting for the receiver.\n\
func (g Greeter) Hello() string {\n\treturn g.Name\n}\n\n\
// Version is the build version.\n\
const Version = \"1.0\"\n";
    let content = render_files(&import_graph(source))[0].content.clone();

    assert!(
        content.contains("// Greeter greets people by name."),
        "struct doc dropped: {content}"
    );
    assert!(
        content.contains("// Hello returns a greeting for the receiver."),
        "method doc dropped: {content}"
    );
    assert!(
        content.contains("// Version is the build version."),
        "const doc dropped: {content}"
    );
}

#[test]
fn field_and_interface_method_docs_round_trip() {
    // Regression: docs on struct fields and interface methods (nested godoc) were uncaptured.
    let source = "package order\n\n\
// Server holds config.\n\
type Server struct {\n\
\t// Port is the listen port.\n\tPort int\n\
}\n\n\
// Handler processes requests.\n\
type Handler interface {\n\
\t// Serve handles one request.\n\tServe(path string) error\n\
}\n";
    let content = render_files(&import_graph(source))[0].content.clone();

    assert!(
        content.contains("// Port is the listen port."),
        "struct field doc dropped: {content}"
    );
    assert!(
        content.contains("// Serve handles one request."),
        "interface method doc dropped: {content}"
    );
}

#[test]
fn doc_only_edit_is_recovered_as_update() {
    // P2: editing ONLY a function's godoc (body/signature unchanged) must produce an UpdateSymbol
    // carrying the new doc, so the change is not silently lost on write-back.
    let mut operations =
        import_operations("package order\n\n// Old summary.\nfunc Run() {\n\treturn\n}\n");
    let graph = materialize_operations(operations.clone());
    let run_id = graph.find_symbol("Run")[0].id;

    let edited = vec![sample_file(
        "package order\n\n// New summary.\nfunc Run() {\n\treturn\n}\n",
    )];
    let recovered = recover_go_operations(&graph, &[run_id], &edited).unwrap();

    assert!(
        recovered.iter().any(|operation| matches!(
            operation,
            Operation::UpdateSymbol { symbol_id, metadata: Some(metadata), .. }
                if *symbol_id == run_id
                    && metadata.get("doc").and_then(|doc| doc.as_str()) == Some("// New summary.")
        )),
        "doc-only edit did not become an UpdateSymbol carrying the new doc: {recovered:?}"
    );

    operations.extend(recovered);
    let rerendered = render_files(&materialize_operations(operations))[0]
        .content
        .clone();
    assert!(rerendered.contains("// New summary."), "{rerendered}");
    assert!(!rerendered.contains("// Old summary."), "{rerendered}");
}

#[test]
fn body_edit_preserves_existing_doc() {
    // Regression: UpdateSymbol metadata replaces the whole blob, so a body-only edit must re-attach
    // the unchanged doc or it would be dropped on the next render.
    let mut operations =
        import_operations("package order\n\n// Keep me.\nfunc Run() {\n\treturn\n}\n");
    let graph = materialize_operations(operations.clone());
    let run_id = graph.find_symbol("Run")[0].id;

    let edited = vec![sample_file(
        "package order\n\n// Keep me.\nfunc Run() {\n\tprintln(\"hi\")\n}\n",
    )];
    operations.extend(recover_go_operations(&graph, &[run_id], &edited).unwrap());

    let rerendered = render_files(&materialize_operations(operations))[0]
        .content
        .clone();
    assert!(
        rerendered.contains("// Keep me."),
        "body edit dropped the unchanged doc: {rerendered}"
    );
    assert!(rerendered.contains("println(\"hi\")"), "{rerendered}");
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
