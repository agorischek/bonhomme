use super::{import_graph, import_operations, materialize_operations, sample_file};
use crate::{diff_slice, recover_operations, render_files};
use bonhomme_core::{Operation, RenderedFile};

#[test]
fn updates_clean_method_body_by_graph_identity() {
    let mut operations = import_operations(
        r#"
export class OrderService {
  displayName(): string {
    return "OrderService";
  }
}
"#,
    );
    let graph = materialize_operations(operations.clone());
    let method_id = graph.find_symbol("displayName")[0].id;
    let edited = vec![sample_file(
        r#"
export class OrderService {
  displayName(): string {
    return "Orders";
  }
}
"#,
    )];

    let recovered = recover_operations(&graph, &[method_id], &edited).unwrap();

    assert!(matches!(
        recovered.as_slice(),
        [Operation::UpdateSymbol {
            symbol_id,
            name: None,
            body: Some(body),
            ..
        }] if *symbol_id == method_id && body.contains("\"Orders\"")
    ));

    operations.extend(recovered);
    let updated = materialize_operations(operations);
    assert!(
        render_files(&updated)[0]
            .content
            .contains("return \"Orders\";")
    );
}

#[test]
fn doc_only_edit_is_recovered_as_update() {
    // P2: editing only a function's TSDoc (signature/body unchanged) must produce an UpdateSymbol
    // carrying the new doc, so the change survives write-back.
    let mut operations =
        import_operations("/** Old summary. */\nexport function run(): void {\n  return;\n}\n");
    let graph = materialize_operations(operations.clone());
    let run_id = graph.find_symbol("run")[0].id;
    let edited = vec![sample_file(
        "/** New summary. */\nexport function run(): void {\n  return;\n}\n",
    )];

    let recovered = recover_operations(&graph, &[run_id], &edited).unwrap();
    assert!(
        recovered.iter().any(|operation| matches!(
            operation,
            Operation::UpdateSymbol { symbol_id, metadata: Some(metadata), .. }
                if *symbol_id == run_id
                    && metadata.get("doc").and_then(|doc| doc.as_str())
                        == Some("/** New summary. */")
        )),
        "doc-only edit did not become an UpdateSymbol carrying the new doc: {recovered:?}"
    );

    operations.extend(recovered);
    let rerendered = render_files(&materialize_operations(operations))[0]
        .content
        .clone();
    assert!(rerendered.contains("/** New summary. */"), "{rerendered}");
    assert!(!rerendered.contains("/** Old summary. */"), "{rerendered}");
}

#[test]
fn body_edit_preserves_existing_doc() {
    // Regression: UpdateSymbol metadata replaces the whole blob, so a body-only edit must re-attach
    // the unchanged doc or it would be dropped on the next render.
    let mut operations =
        import_operations("/** Keep me. */\nexport function run(): void {\n  return;\n}\n");
    let graph = materialize_operations(operations.clone());
    let run_id = graph.find_symbol("run")[0].id;
    let edited = vec![sample_file(
        "/** Keep me. */\nexport function run(): void {\n  console.log(\"hi\");\n}\n",
    )];

    operations.extend(recover_operations(&graph, &[run_id], &edited).unwrap());
    let rerendered = render_files(&materialize_operations(operations))[0]
        .content
        .clone();
    assert!(
        rerendered.contains("/** Keep me. */"),
        "body edit dropped the unchanged doc: {rerendered}"
    );
    assert!(rerendered.contains("console.log"), "{rerendered}");
}

#[test]
fn renames_clean_method_by_structure() {
    let graph = import_graph(
        r#"
export class OrderService {
  displayName(): string {
    return "OrderService";
  }
}
"#,
    );
    let method_id = graph.find_symbol("displayName")[0].id;
    let edited = vec![sample_file(
        r#"
export class OrderService {
  label(): string {
    return "OrderService";
  }
}
"#,
    )];

    let recovered = recover_operations(&graph, &[method_id], &edited).unwrap();

    assert!(matches!(
        recovered.as_slice(),
        [Operation::UpdateSymbol {
            symbol_id,
            name: Some(name),
            ..
        }] if *symbol_id == method_id && name == "label"
    ));
}

#[test]
fn creates_and_deletes_clean_methods_without_guessing_rename() {
    let mut operations = import_operations(
        r#"
export class OrderService {
  keep(): string {
    return "keep";
  }

  remove(): string {
    return "old";
  }
}
"#,
    );
    let graph = materialize_operations(operations.clone());
    let remove_id = graph.find_symbol("remove")[0].id;
    let edited = vec![sample_file(
        r#"
export class OrderService {
  keep(): string {
    return "keep";
  }

  added(): string {
    const value = "new";
    return value;
  }
}
"#,
    )];

    let recovered = recover_operations(&graph, &[], &edited).unwrap();

    assert!(matches!(
        recovered.as_slice(),
        [
            Operation::DeleteSymbol { symbol_id },
            Operation::CreateSymbol { name, .. }
        ] if *symbol_id == remove_id && name == "added"
    ));

    operations.extend(recovered);
    let updated = materialize_operations(operations);
    assert!(updated.find_symbol("remove").is_empty());
    assert_eq!(updated.find_symbol("added").len(), 1);
}

#[test]
fn updates_call_references_when_clean_method_body_changes() {
    let mut operations = import_operations(
        r#"
export class OrderService {
  displayName(): string {
    return "OrderService";
  }

  listOrders(): string[] {
    return ["one"];
  }

  summary(): string {
    return this.displayName();
  }
}
"#,
    );
    let graph = materialize_operations(operations.clone());
    let display_name_id = graph.find_symbol("displayName")[0].id;
    let list_orders_id = graph.find_symbol("listOrders")[0].id;
    let summary_id = graph.find_symbol("summary")[0].id;
    assert_eq!(
        graph.find_callees(summary_id, "calls")[0].id,
        display_name_id
    );
    let edited = vec![sample_file(
        r#"
export class OrderService {
  displayName(): string {
    return "OrderService";
  }

  listOrders(): string[] {
    return ["one"];
  }

  summary(): string {
    return this.listOrders().join(",");
  }
}
"#,
    )];

    let recovered = recover_operations(&graph, &[summary_id], &edited).unwrap();

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
fn deletes_references_before_deleted_symbols() {
    let mut operations = import_operations(
        r#"
export class OrderService {
  displayName(): string {
    return "OrderService";
  }

  summary(): string {
    return this.displayName();
  }
}
"#,
    );
    let graph = materialize_operations(operations.clone());
    let summary_id = graph.find_symbol("summary")[0].id;
    assert_eq!(graph.find_references(summary_id).len(), 1);
    let edited = vec![sample_file(
        r#"
export class OrderService {
  displayName(): string {
    return "OrderService";
  }
}
"#,
    )];

    let recovered = recover_operations(&graph, &[], &edited).unwrap();

    assert!(matches!(
        recovered.as_slice(),
        [
            Operation::DeleteReference { .. },
            Operation::DeleteSymbol { symbol_id }
        ] if *symbol_id == summary_id
    ));

    operations.extend(recovered);
    let updated = materialize_operations(operations);
    assert!(updated.find_symbol("summary").is_empty());
}

#[test]
fn rejects_ambiguous_multi_method_identity_recovery() {
    let graph = import_graph(
        r#"
export class OrderService {
  alpha(): string {
    return "north";
  }

  beta(): string {
    return "south";
  }
}
"#,
    );
    let class_id = graph.find_symbol("OrderService")[0].id;
    let edited = vec![sample_file(
        r#"
export class OrderService {
  gamma(): string {
    return "warehouse";
  }

  delta(): string {
    return "billing";
  }
}
"#,
    )];

    let error = recover_operations(&graph, &[class_id], &edited)
        .expect_err("ambiguous multi-rename recovery must reject")
        .to_string();

    assert!(error.contains("ambiguous structural method identity recovery"));
    assert!(error.contains("class OrderService"));
    assert!(error.contains("refusing to guess"));
    assert!(error.contains("alpha") && error.contains("beta"));
    assert!(error.contains("gamma") && error.contains("delta"));
}

#[test]
fn matches_comment_diff_for_existing_slice_edits() {
    let graph = import_graph(
        r#"
export class OrderService {
  displayName(): string {
    return "OrderService";
  }
}
"#,
    );
    let file_id = graph.root_symbols()[0].id;
    let class_id = graph.find_symbol("OrderService")[0].id;
    let method_id = graph.find_symbol("displayName")[0].id;
    let original = vec![RenderedFile {
        path: "src/Sample.ts".to_string(),
        content: format!(
            r#"// bonhomme:file={file_id}

export class OrderService /* bonhomme:symbol={class_id} */ {{
  displayName(): string /* bonhomme:symbol={method_id} */ {{
    return "OrderService";
  }}
}}
"#
        ),
    }];
    let modified = vec![RenderedFile {
        path: original[0].path.clone(),
        content: original[0]
            .content
            .replace("return \"OrderService\";", "return \"Orders\";"),
    }];

    let legacy = diff_slice(&original, &modified).unwrap();
    let structural = recover_operations(&graph, &[], &modified).unwrap();

    assert_eq!(structural, legacy);
}
