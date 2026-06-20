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
    let original = render_files(&graph);
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
