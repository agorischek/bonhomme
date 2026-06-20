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
    pub(crate) types: BTreeMap<String, Uuid>,
    pub(crate) functions: BTreeMap<String, Uuid>,
    pub(crate) methods: BTreeMap<(String, String), Uuid>,
    pub(crate) calls: BTreeMap<Uuid, Vec<CallTarget>>,
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
        for declaration in &file.declarations {
            match declaration.kind.as_str() {
                "struct" | "interface" | "type" => {
                    indexes
                        .types
                        .insert(declaration.name.clone(), type_id(&declaration.name));
                }
                "function" => {
                    indexes
                        .functions
                        .insert(declaration.name.clone(), function_id(&declaration.name));
                }
                "method" => {
                    if let Some(receiver) = &declaration.receiver {
                        indexes.methods.insert(
                            (receiver.clone(), declaration.name.clone()),
                            method_id(receiver, &declaration.name),
                        );
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
        name: file_name(&file.path),
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
    for declaration in &file.declarations {
        match declaration.kind.as_str() {
            "struct" => operations.extend(struct_operations(file_id, declaration)),
            "interface" => operations.extend(interface_operations(file_id, declaration)),
            "function" => {
                let symbol_id = function_id(&declaration.name);
                indexes.calls.insert(symbol_id, declaration.calls.clone());
                operations.push(Operation::CreateSymbol {
                    symbol_id,
                    parent_id: Some(file_id),
                    kind: "function".to_string(),
                    name: declaration.name.clone(),
                    body: declaration.body.clone(),
                    metadata: json!({
                        "signature": declaration.signature.as_deref().unwrap_or(""),
                        "path": file.path,
                    }),
                });
            }
            "const" | "var" | "type" => {
                operations.push(Operation::CreateSymbol {
                    symbol_id: value_id(&declaration.kind, &declaration.name),
                    parent_id: Some(file_id),
                    kind: declaration.kind.clone(),
                    name: declaration.name.clone(),
                    body: None,
                    metadata: json!({
                        "declaration": declaration.declaration.as_deref().unwrap_or(""),
                        "path": file.path,
                    }),
                });
            }
            "method" => {
                let Some(receiver) = &declaration.receiver else {
                    continue;
                };
                indexes
                    .types
                    .get(receiver)
                    .with_context(|| format!("Go receiver type {receiver} does not exist"))?;
            }
            _ => {}
        }
    }
    Ok(operations)
}

fn method_operations(file: &ParsedFile, indexes: &mut ImportIndexes) -> Result<Vec<Operation>> {
    let mut operations = Vec::new();
    for declaration in &file.declarations {
        if declaration.kind != "method" {
            continue;
        }
        let receiver = declaration
            .receiver
            .as_ref()
            .context("Go method declaration is missing receiver")?;
        let parent_id = *indexes
            .types
            .get(receiver)
            .with_context(|| format!("Go receiver type {receiver} does not exist"))?;
        let symbol_id = method_id(receiver, &declaration.name);
        indexes.calls.insert(symbol_id, declaration.calls.clone());
        operations.push(Operation::CreateSymbol {
            symbol_id,
            parent_id: Some(parent_id),
            kind: "method".to_string(),
            name: declaration.name.clone(),
            body: declaration.body.clone(),
            metadata: json!({
                "signature": declaration.signature.as_deref().unwrap_or(""),
                "receiver": receiver,
                "path": file.path,
            }),
        });
    }
    Ok(operations)
}

fn struct_operations(file_id: Uuid, declaration: &Declaration) -> Vec<Operation> {
    let symbol_id = type_id(&declaration.name);
    let mut operations = vec![Operation::CreateSymbol {
        symbol_id,
        parent_id: Some(file_id),
        kind: "struct".to_string(),
        name: declaration.name.clone(),
        body: None,
        metadata: json!({
            "declaration": declaration.declaration.as_deref().unwrap_or(""),
        }),
    }];
    for field in &declaration.fields {
        operations.push(Operation::CreateSymbol {
            symbol_id: field_id(&declaration.name, &field.name),
            parent_id: Some(symbol_id),
            kind: "field".to_string(),
            name: field.name.clone(),
            body: None,
            metadata: json!({"declaration": field.declaration}),
        });
    }
    operations
}

fn interface_operations(file_id: Uuid, declaration: &Declaration) -> Vec<Operation> {
    let symbol_id = type_id(&declaration.name);
    let mut operations = vec![Operation::CreateSymbol {
        symbol_id,
        parent_id: Some(file_id),
        kind: "interface".to_string(),
        name: declaration.name.clone(),
        body: None,
        metadata: json!({
            "declaration": declaration.declaration.as_deref().unwrap_or(""),
        }),
    }];
    for method in &declaration.methods {
        operations.push(Operation::CreateSymbol {
            symbol_id: interface_method_id(&declaration.name, &method.name),
            parent_id: Some(symbol_id),
            kind: "method".to_string(),
            name: method.name.clone(),
            body: None,
            metadata: json!({"signature": method.signature}),
        });
    }
    operations
}

pub(crate) fn reference_operations(indexes: &ImportIndexes) -> Vec<Operation> {
    let mut seen = BTreeSet::new();
    let mut operations = Vec::new();
    for (from_symbol_id, calls) in &indexes.calls {
        for call in calls {
            let Some(to_symbol_id) = resolve_call(indexes, call) else {
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

pub(crate) fn resolve_call(indexes: &ImportIndexes, call: &CallTarget) -> Option<Uuid> {
    match call.kind.as_str() {
        "function" => indexes.functions.get(&call.name).copied(),
        "method" => indexes
            .methods
            .get(&(call.receiver.as_ref()?.clone(), call.name.clone()))
            .copied(),
        _ => None,
    }
}

pub(crate) fn file_id(path: &str) -> Uuid {
    stable_go_uuid(&format!("file:{path}"))
}

pub(crate) fn type_id(name: &str) -> Uuid {
    stable_go_uuid(&format!("type:{name}"))
}

pub(crate) fn function_id(name: &str) -> Uuid {
    stable_go_uuid(&format!("function:{name}"))
}

pub(crate) fn method_id(receiver: &str, name: &str) -> Uuid {
    stable_go_uuid(&format!("method:{receiver}:{name}"))
}

pub(crate) fn field_id(owner: &str, name: &str) -> Uuid {
    stable_go_uuid(&format!("field:{owner}:{name}"))
}

pub(crate) fn interface_method_id(owner: &str, name: &str) -> Uuid {
    stable_go_uuid(&format!("interface-method:{owner}:{name}"))
}

pub(crate) fn value_id(kind: &str, name: &str) -> Uuid {
    stable_go_uuid(&format!("{kind}:{name}"))
}

fn file_name(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}
