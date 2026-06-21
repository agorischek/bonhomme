pub(crate) mod calls;

use crate::{
    oxc_parse::metadata_with_doc,
    parse::{
        ParsedClass, ParsedClassMemberIndex, ParsedFile, ParsedFunction, ParsedMethod,
        ParsedProperty, ParsedTopLevelIndex, parse_file,
    },
    scanner::stable_import_uuid,
};
use anyhow::Result;
use bonhomme_core::{Operation, RenderedFile};
use serde_json::json;
use uuid::Uuid;

use calls::{CallsBySymbol, ImportIndexes, import_references};

pub fn import_typescript_files(files: &[RenderedFile]) -> Result<Vec<Operation>> {
    let parsed = files.iter().map(parse_file).collect::<Result<Vec<_>>>()?;
    Ok(create_operations_from_parsed_files(&parsed))
}

pub(crate) fn create_operations_from_parsed_files(files: &[ParsedFile]) -> Vec<Operation> {
    let mut operations = Vec::new();
    let mut indexes = ImportIndexes::default();

    for file in files {
        let file_id = resolved_file_id(file);
        operations.push(Operation::CreateSymbol {
            symbol_id: file_id,
            parent_id: None,
            kind: "file".to_string(),
            name: file.path.clone(),
            body: None,
            metadata: json!({
                "handler": "typescript",
                "path": file.path,
                "preamble": file.preamble
            }),
        });

        for operation in create_top_level_operations(file, file_id, &mut indexes) {
            operations.push(operation);
        }
    }

    operations.extend(import_references(&indexes));
    operations
}

fn create_top_level_operations(
    file: &ParsedFile,
    file_id: Uuid,
    indexes: &mut ImportIndexes,
) -> Vec<Operation> {
    let mut operations = Vec::new();
    for entry in &file.top_level_order {
        let mut calls = CallsBySymbol::new();
        let top_level_operations = match *entry {
            ParsedTopLevelIndex::Class(index) => {
                create_class_operations(file, file_id, &file.classes[index], &mut calls)
            }
            ParsedTopLevelIndex::Function(index) => {
                vec![create_function_operation(
                    file,
                    file_id,
                    &file.functions[index],
                    &mut calls,
                )]
            }
        };

        for operation in top_level_operations {
            indexes.index_created_symbol(&operation, &calls);
            operations.push(operation);
        }
    }
    operations
}

pub(crate) fn resolved_file_id(file: &ParsedFile) -> Uuid {
    file.file_symbol_id
        .unwrap_or_else(|| stable_import_uuid(&format!("file:{}", file.path)))
}

pub(crate) fn resolved_class_id(file: &ParsedFile, class: &ParsedClass) -> Uuid {
    class
        .symbol_id
        .unwrap_or_else(|| stable_import_uuid(&format!("class:{}:{}", file.path, class.name)))
}

pub(crate) fn resolved_function_id(file: &ParsedFile, function: &ParsedFunction) -> Uuid {
    function
        .symbol_id
        .unwrap_or_else(|| stable_import_uuid(&format!("function:{}:{}", file.path, function.name)))
}

pub(crate) fn resolved_method_id(file: &ParsedFile, class_id: Uuid, method: &ParsedMethod) -> Uuid {
    method.symbol_id.unwrap_or_else(|| {
        stable_import_uuid(&format!(
            "{}:{}:{}:{}",
            method.kind, file.path, class_id, method.name
        ))
    })
}

pub(crate) fn resolved_property_id(
    file: &ParsedFile,
    class_id: Uuid,
    property: &ParsedProperty,
) -> Uuid {
    property.symbol_id.unwrap_or_else(|| {
        stable_import_uuid(&format!(
            "property:{}:{}:{}",
            file.path, class_id, property.name
        ))
    })
}

pub(crate) fn class_metadata(class: &ParsedClass) -> serde_json::Value {
    metadata_with_doc(
        json!({
            "exported": class.declaration.split_whitespace().next() == Some("export"),
            "declaration": class.declaration
        }),
        class.doc.as_deref(),
    )
}

pub(crate) fn function_metadata(function: &ParsedFunction) -> serde_json::Value {
    metadata_with_doc(
        json!({
            "declaration": function.signature,
            "exported": signature_is_exported(&function.signature)
        }),
        function.doc.as_deref(),
    )
}

pub(crate) fn method_metadata(method: &ParsedMethod) -> serde_json::Value {
    metadata_with_doc(
        json!({
            "signature": method.signature,
            "methodKind": method.method_kind,
            "static": method.is_static
        }),
        method.doc.as_deref(),
    )
}

pub(crate) fn property_metadata(property: &ParsedProperty) -> serde_json::Value {
    metadata_with_doc(
        json!({ "declaration": property.declaration }),
        property.doc.as_deref(),
    )
}

fn create_class_operations(
    file: &ParsedFile,
    file_id: Uuid,
    class: &ParsedClass,
    calls: &mut CallsBySymbol,
) -> Vec<Operation> {
    let class_id = resolved_class_id(file, class);
    let mut operations = vec![Operation::CreateSymbol {
        symbol_id: class_id,
        parent_id: Some(file_id),
        kind: "class".to_string(),
        name: class.name.clone(),
        body: None,
        metadata: class_metadata(class),
    }];

    for member in &class.member_order {
        match *member {
            ParsedClassMemberIndex::Method(index) => {
                operations.push(create_method_operation(
                    file,
                    class_id,
                    &class.methods[index],
                    calls,
                ));
            }
            ParsedClassMemberIndex::Property(index) => {
                operations.push(create_property_operation(
                    file,
                    class_id,
                    &class.properties[index],
                ));
            }
        }
    }

    operations
}

fn create_function_operation(
    file: &ParsedFile,
    file_id: Uuid,
    function: &ParsedFunction,
    calls: &mut CallsBySymbol,
) -> Operation {
    let function_id = resolved_function_id(file, function);
    calls.insert(function_id, function.calls.clone());
    Operation::CreateSymbol {
        symbol_id: function_id,
        parent_id: Some(file_id),
        kind: "function".to_string(),
        name: function.name.clone(),
        body: Some(function.body.clone()),
        metadata: function_metadata(function),
    }
}

fn create_method_operation(
    file: &ParsedFile,
    class_id: Uuid,
    method: &ParsedMethod,
    calls: &mut CallsBySymbol,
) -> Operation {
    let method_id = resolved_method_id(file, class_id, method);
    calls.insert(method_id, method.calls.clone());
    Operation::CreateSymbol {
        symbol_id: method_id,
        parent_id: Some(class_id),
        kind: method.kind.clone(),
        name: method.name.clone(),
        body: Some(method.body.clone()),
        metadata: method_metadata(method),
    }
}

fn create_property_operation(
    file: &ParsedFile,
    class_id: Uuid,
    property: &ParsedProperty,
) -> Operation {
    Operation::CreateSymbol {
        symbol_id: resolved_property_id(file, class_id, property),
        parent_id: Some(class_id),
        kind: "property".to_string(),
        name: property.name.clone(),
        body: None,
        metadata: property_metadata(property),
    }
}

pub(crate) fn signature_is_exported(signature: &str) -> bool {
    signature.split_whitespace().next() == Some("export")
}
