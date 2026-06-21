use crate::{
    model::{CallTarget, Declaration, ParsedFile, ParsedPackage},
    toolchain::parse_go_files,
};
use anyhow::{Context, Result};
use bonhomme_core::{Operation, RenderedFile};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

const CALLS_KIND: &str = "calls";

#[derive(Default)]
pub(crate) struct ImportIndexes {
    pub(crate) types: BTreeMap<(String, String), Uuid>,
    types_by_file: BTreeMap<(String, String, String), Uuid>,
    pub(crate) functions: BTreeMap<(String, String), Uuid>,
    pub(crate) methods: BTreeMap<(String, String, String), Uuid>,
    method_counts: BTreeMap<(String, String, String), usize>,
    pub(crate) calls: BTreeMap<Uuid, (String, Vec<CallTarget>)>,
}

pub fn import_go_files(files: &[RenderedFile]) -> Result<Vec<Operation>> {
    let parsed = parse_go_files(files)?;
    operations_from_parsed_package(&parsed)
}

pub(crate) fn operations_from_parsed_package(parsed: &ParsedPackage) -> Result<Vec<Operation>> {
    let mut indexes = ImportIndexes::default();
    index_package(parsed, &mut indexes);

    let mut operations = Vec::new();
    for file in &parsed.files {
        operations.push(file_operation(file));
    }
    for file in &parsed.files {
        operations.extend(non_method_operations(file, &mut indexes)?);
    }
    for file in &parsed.files {
        operations.extend(method_operations(file, &mut indexes)?);
    }
    operations.extend(reference_operations(&indexes));
    Ok(operations)
}

pub(crate) fn stable_go_uuid(seed: &str) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("https://bonhomme.local/go/{seed}").as_bytes(),
    )
}

pub(crate) fn stable_reference_uuid(from_symbol_id: Uuid, to_symbol_id: Uuid, kind: &str) -> Uuid {
    stable_go_uuid(&format!("reference:{from_symbol_id}:{to_symbol_id}:{kind}"))
}

fn index_package(parsed: &ParsedPackage, indexes: &mut ImportIndexes) {
    for file in &parsed.files {
        let scope = package_scope(file);
        for declaration in &file.declarations {
            if declaration.kind == "method"
                && let Some(receiver) = &declaration.receiver
            {
                *indexes
                    .method_counts
                    .entry((scope.clone(), receiver.clone(), declaration.name.clone()))
                    .or_insert(0) += 1;
            }
            match declaration.kind.as_str() {
                "struct" | "interface" | "type" => {
                    indexes.types.insert(
                        (scope.clone(), declaration.name.clone()),
                        type_id(&scope, &file.path, &declaration.name),
                    );
                    indexes.types_by_file.insert(
                        (scope.clone(), file.path.clone(), declaration.name.clone()),
                        type_id(&scope, &file.path, &declaration.name),
                    );
                }
                "function" => {
                    indexes.functions.insert(
                        (scope.clone(), declaration.name.clone()),
                        function_id(&scope, &file.path, &declaration.name),
                    );
                }
                "method" => {
                    if let Some(receiver) = &declaration.receiver {
                        let key = (scope.clone(), receiver.clone(), declaration.name.clone());
                        if indexes.method_counts.get(&key).copied().unwrap_or_default() == 1
                            && let Some(parent_id) =
                                receiver_type_id(indexes, &scope, &file.path, receiver)
                        {
                            indexes
                                .methods
                                .insert(key, method_id(parent_id, &file.path, &declaration.name));
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

fn file_operation(file: &ParsedFile) -> Operation {
    Operation::CreateSymbol {
        symbol_id: file_id(&file.path),
        parent_id: None,
        kind: "file".to_string(),
        name: file.path.clone(),
        body: None,
        metadata: json!({
            "handler": "go",
            "path": file.path,
            "package": file.package_name,
            "imports": file.imports,
        }),
    }
}

fn non_method_operations(file: &ParsedFile, indexes: &mut ImportIndexes) -> Result<Vec<Operation>> {
    let mut operations = Vec::new();
    let file_id = file_id(&file.path);
    let scope = package_scope(file);
    for declaration in &file.declarations {
        match declaration.kind.as_str() {
            "struct" => {
                operations.extend(struct_operations(file_id, &scope, &file.path, declaration))
            }
            "interface" => operations.extend(interface_operations(
                file_id,
                &scope,
                &file.path,
                declaration,
            )),
            "function" => {
                let symbol_id = function_id(&scope, &file.path, &declaration.name);
                indexes
                    .calls
                    .insert(symbol_id, (scope.clone(), declaration.calls.clone()));
                operations.push(Operation::CreateSymbol {
                    symbol_id,
                    parent_id: Some(file_id),
                    kind: "function".to_string(),
                    name: declaration.name.clone(),
                    body: declaration.body.clone(),
                    metadata: with_doc(
                        json!({
                            "signature": declaration.signature.as_deref().unwrap_or(""),
                            "path": file.path,
                        }),
                        declaration,
                    ),
                });
            }
            "const" | "var" | "type" => {
                operations.push(Operation::CreateSymbol {
                    symbol_id: value_id(&scope, &file.path, &declaration.kind, &declaration.name),
                    parent_id: Some(file_id),
                    kind: declaration.kind.clone(),
                    name: declaration.name.clone(),
                    body: None,
                    metadata: with_doc(
                        json!({
                            "declaration": declaration.declaration.as_deref().unwrap_or(""),
                            "path": file.path,
                        }),
                        declaration,
                    ),
                });
            }
            "method" => {}
            _ => {}
        }
    }
    Ok(operations)
}

fn method_operations(file: &ParsedFile, indexes: &mut ImportIndexes) -> Result<Vec<Operation>> {
    let mut operations = Vec::new();
    let scope = package_scope(file);
    for declaration in &file.declarations {
        if declaration.kind != "method" {
            continue;
        }
        let receiver = declaration
            .receiver
            .as_ref()
            .context("Go method declaration is missing receiver")?;
        let parent_id = method_parent_id(
            indexes,
            file_id(&file.path),
            &scope,
            &file.path,
            receiver,
            &declaration.name,
        );
        let symbol_id = method_id(parent_id, &file.path, &declaration.name);
        indexes
            .calls
            .insert(symbol_id, (scope.clone(), declaration.calls.clone()));
        operations.push(Operation::CreateSymbol {
            symbol_id,
            parent_id: Some(parent_id),
            kind: "method".to_string(),
            name: declaration.name.clone(),
            body: declaration.body.clone(),
            metadata: with_doc(
                json!({
                    "signature": declaration.signature.as_deref().unwrap_or(""),
                    "receiver": receiver,
                    "path": file.path,
                }),
                declaration,
            ),
        });
    }
    Ok(operations)
}

/// Attach a declaration's godoc comment (`// …` above it) as `doc` metadata, so it renders back.
fn with_doc(metadata: serde_json::Value, declaration: &Declaration) -> serde_json::Value {
    metadata_with_doc(metadata, declaration.doc.as_deref())
}

/// Attach a raw godoc block as `doc` metadata when present and non-empty. Shared by declarations,
/// struct fields, and interface methods so each documentable node renders its doc back. Also used by
/// the recover path so an edited slice's docs survive (and doc-only edits become `UpdateSymbol`s).
pub(crate) fn metadata_with_doc(
    mut metadata: serde_json::Value,
    doc: Option<&str>,
) -> serde_json::Value {
    if let Some(doc) = doc.filter(|doc| !doc.is_empty()) {
        metadata["doc"] = json!(doc);
    }
    metadata
}

fn struct_operations(
    file_id: Uuid,
    scope: &str,
    path: &str,
    declaration: &Declaration,
) -> Vec<Operation> {
    let symbol_id = type_id(scope, path, &declaration.name);
    let mut operations = vec![Operation::CreateSymbol {
        symbol_id,
        parent_id: Some(file_id),
        kind: "struct".to_string(),
        name: declaration.name.clone(),
        body: None,
        metadata: with_doc(
            json!({
                "declaration": declaration.declaration.as_deref().unwrap_or(""),
            }),
            declaration,
        ),
    }];
    for field in &declaration.fields {
        operations.push(Operation::CreateSymbol {
            symbol_id: field_id(symbol_id, &field.name),
            parent_id: Some(symbol_id),
            kind: "field".to_string(),
            name: field.name.clone(),
            body: None,
            metadata: metadata_with_doc(
                json!({"declaration": field.declaration}),
                field.doc.as_deref(),
            ),
        });
    }
    operations
}

fn interface_operations(
    file_id: Uuid,
    scope: &str,
    path: &str,
    declaration: &Declaration,
) -> Vec<Operation> {
    let symbol_id = type_id(scope, path, &declaration.name);
    let mut operations = vec![Operation::CreateSymbol {
        symbol_id,
        parent_id: Some(file_id),
        kind: "interface".to_string(),
        name: declaration.name.clone(),
        body: None,
        metadata: with_doc(
            json!({
                "declaration": declaration.declaration.as_deref().unwrap_or(""),
            }),
            declaration,
        ),
    }];
    for method in &declaration.methods {
        operations.push(Operation::CreateSymbol {
            symbol_id: interface_method_id(symbol_id, &method.name),
            parent_id: Some(symbol_id),
            kind: "method".to_string(),
            name: method.name.clone(),
            body: None,
            metadata: metadata_with_doc(
                json!({"signature": method.signature}),
                method.doc.as_deref(),
            ),
        });
    }
    operations
}

pub(crate) fn reference_operations(indexes: &ImportIndexes) -> Vec<Operation> {
    let mut seen = BTreeSet::new();
    let mut operations = Vec::new();
    for (from_symbol_id, (scope, calls)) in &indexes.calls {
        for call in calls {
            let Some(to_symbol_id) = resolve_call(indexes, scope, call) else {
                continue;
            };
            if to_symbol_id == *from_symbol_id
                || !seen.insert((*from_symbol_id, to_symbol_id, CALLS_KIND))
            {
                continue;
            }
            operations.push(Operation::CreateReference {
                reference_id: stable_reference_uuid(*from_symbol_id, to_symbol_id, CALLS_KIND),
                from_symbol_id: *from_symbol_id,
                to_symbol_id,
                kind: CALLS_KIND.to_string(),
            });
        }
    }
    operations
}

pub(crate) fn resolve_call(
    indexes: &ImportIndexes,
    scope: &str,
    call: &CallTarget,
) -> Option<Uuid> {
    match call.kind.as_str() {
        "function" => indexes
            .functions
            .get(&(scope.to_string(), call.name.clone()))
            .copied(),
        "method" => indexes
            .methods
            .get(&(
                scope.to_string(),
                call.receiver.as_ref()?.clone(),
                call.name.clone(),
            ))
            .copied(),
        _ => None,
    }
}

pub(crate) fn file_id(path: &str) -> Uuid {
    stable_go_uuid(&format!("file:{path}"))
}

pub(crate) fn package_scope(file: &ParsedFile) -> String {
    scope_from_path(&file.path, &file.package_name)
}

pub(crate) fn scope_from_path(path: &str, package_name: &str) -> String {
    let directory = path.rsplit_once('/').map(|(dir, _)| dir).unwrap_or(".");
    format!("{directory}:{package_name}")
}

pub(crate) fn type_id(scope: &str, path: &str, name: &str) -> Uuid {
    stable_go_uuid(&format!("type:{scope}:{path}:{name}"))
}

pub(crate) fn function_id(scope: &str, path: &str, name: &str) -> Uuid {
    stable_go_uuid(&format!("function:{scope}:{path}:{name}"))
}

fn receiver_type_id(
    indexes: &ImportIndexes,
    scope: &str,
    path: &str,
    receiver: &str,
) -> Option<Uuid> {
    indexes
        .types_by_file
        .get(&(scope.to_string(), path.to_string(), receiver.to_string()))
        .or_else(|| {
            indexes
                .types
                .get(&(scope.to_string(), receiver.to_string()))
        })
        .copied()
}

fn method_parent_id(
    indexes: &ImportIndexes,
    file_id: Uuid,
    scope: &str,
    path: &str,
    receiver: &str,
    name: &str,
) -> Uuid {
    let key = (scope.to_string(), receiver.to_string(), name.to_string());
    if indexes.method_counts.get(&key).copied().unwrap_or_default() > 1 {
        return file_id;
    }
    receiver_type_id(indexes, scope, path, receiver).unwrap_or(file_id)
}

pub(crate) fn method_id(parent_id: Uuid, path: &str, name: &str) -> Uuid {
    stable_go_uuid(&format!("method:{parent_id}:{path}:{name}"))
}

pub(crate) fn field_id(owner_id: Uuid, name: &str) -> Uuid {
    stable_go_uuid(&format!("field:{owner_id}:{name}"))
}

pub(crate) fn interface_method_id(owner_id: Uuid, name: &str) -> Uuid {
    stable_go_uuid(&format!("interface-method:{owner_id}:{name}"))
}

pub(crate) fn value_id(scope: &str, path: &str, kind: &str, name: &str) -> Uuid {
    stable_go_uuid(&format!("{kind}:{scope}:{path}:{name}"))
}
