use super::{
    Plan,
    base::{BaseSymbol, children_of_kind},
    delete_subtree,
    matcher::match_by_name,
    queue_delete,
};
use crate::{
    import::{field_id, interface_method_id, metadata_with_doc, package_scope, type_id},
    model::{Declaration, ParsedFile},
};
use anyhow::Result;
use bonhomme_core::{Operation, SemanticGraph};
use serde_json::json;
use uuid::Uuid;

pub(super) fn recover_types(
    base: &SemanticGraph,
    file_id: Uuid,
    file: &ParsedFile,
    plan: &mut Plan,
) -> Result<()> {
    for kind in ["struct", "interface"] {
        let base_symbols = children_of_kind(base, file_id, kind);
        let edited = file
            .declarations
            .iter()
            .filter(|declaration| declaration.kind == kind)
            .collect::<Vec<_>>();
        let matches = match_by_name(&base_symbols, &edited);
        for (base_index, edited_index) in matches.matched {
            let base_symbol = base_symbols[base_index].clone();
            let edited_symbol = edited[edited_index];
            recover_type_children(base, &base_symbol, edited_symbol, plan)?;
            update_type_declaration_if_needed(&base_symbol, edited_symbol, plan);
        }
        for edited_index in matches.added {
            create_type(file_id, file, edited[edited_index], plan);
        }
        for base_index in matches.deleted {
            delete_subtree(base, base_symbols[base_index].id, plan);
        }
    }
    Ok(())
}

fn update_type_declaration_if_needed(
    base_symbol: &BaseSymbol,
    edited_symbol: &Declaration,
    plan: &mut Plan,
) {
    let declaration = edited_symbol.declaration.clone().unwrap_or_default();
    if base_symbol.name != edited_symbol.name
        || base_symbol.declaration != declaration
        || base_symbol.doc.as_deref() != edited_symbol.doc.as_deref()
    {
        plan.symbol_edits.push(Operation::UpdateSymbol {
            symbol_id: base_symbol.id,
            name: (base_symbol.name != edited_symbol.name).then(|| edited_symbol.name.clone()),
            body: None,
            metadata: Some(metadata_with_doc(
                json!({ "declaration": declaration }),
                edited_symbol.doc.as_deref(),
            )),
        });
    }
}

fn recover_type_children(
    base: &SemanticGraph,
    base_type: &BaseSymbol,
    edited_type: &Declaration,
    plan: &mut Plan,
) -> Result<()> {
    match edited_type.kind.as_str() {
        "struct" => recover_fields(base, base_type, edited_type, plan),
        "interface" => recover_interface_methods(base, base_type, edited_type, plan),
        _ => Ok(()),
    }
}

fn recover_fields(
    base: &SemanticGraph,
    base_type: &BaseSymbol,
    edited_type: &Declaration,
    plan: &mut Plan,
) -> Result<()> {
    let base_fields = children_of_kind(base, base_type.id, "field");
    let edited = edited_type.fields.iter().collect::<Vec<_>>();
    let matches = match_by_name(&base_fields, &edited);
    for (base_index, edited_index) in matches.matched {
        let base_field = &base_fields[base_index];
        let edited_field = edited[edited_index];
        if base_field.declaration != edited_field.declaration
            || base_field.doc.as_deref() != edited_field.doc.as_deref()
        {
            plan.symbol_edits.push(Operation::UpdateSymbol {
                symbol_id: base_field.id,
                name: None,
                body: None,
                metadata: Some(metadata_with_doc(
                    json!({"declaration": edited_field.declaration}),
                    edited_field.doc.as_deref(),
                )),
            });
        }
    }
    for edited_index in matches.added {
        let edited_field = edited[edited_index];
        plan.symbol_edits.push(Operation::CreateSymbol {
            symbol_id: field_id(base_type.id, &edited_field.name),
            parent_id: Some(base_type.id),
            kind: "field".to_string(),
            name: edited_field.name.clone(),
            body: None,
            metadata: metadata_with_doc(
                json!({"declaration": edited_field.declaration}),
                edited_field.doc.as_deref(),
            ),
        });
    }
    for base_index in matches.deleted {
        queue_delete(base_fields[base_index].id, plan);
    }
    Ok(())
}

fn recover_interface_methods(
    base: &SemanticGraph,
    base_type: &BaseSymbol,
    edited_type: &Declaration,
    plan: &mut Plan,
) -> Result<()> {
    let base_methods = children_of_kind(base, base_type.id, "method")
        .into_iter()
        .filter(|method| method.body.is_empty())
        .collect::<Vec<_>>();
    let edited = edited_type.methods.iter().collect::<Vec<_>>();
    let matches = match_by_name(&base_methods, &edited);
    for (base_index, edited_index) in matches.matched {
        let base_method = &base_methods[base_index];
        let edited_method = edited[edited_index];
        if base_method.signature != edited_method.signature
            || base_method.doc.as_deref() != edited_method.doc.as_deref()
        {
            plan.symbol_edits.push(Operation::UpdateSymbol {
                symbol_id: base_method.id,
                name: None,
                body: None,
                metadata: Some(metadata_with_doc(
                    json!({"signature": edited_method.signature}),
                    edited_method.doc.as_deref(),
                )),
            });
        }
    }
    for edited_index in matches.added {
        let edited_method = edited[edited_index];
        plan.symbol_edits.push(Operation::CreateSymbol {
            symbol_id: interface_method_id(base_type.id, &edited_method.name),
            parent_id: Some(base_type.id),
            kind: "method".to_string(),
            name: edited_method.name.clone(),
            body: None,
            metadata: metadata_with_doc(
                json!({"signature": edited_method.signature}),
                edited_method.doc.as_deref(),
            ),
        });
    }
    for base_index in matches.deleted {
        queue_delete(base_methods[base_index].id, plan);
    }
    Ok(())
}

fn create_type(file_id: Uuid, file: &ParsedFile, declaration: &Declaration, plan: &mut Plan) {
    let scope = package_scope(file);
    let symbol_id = type_id(&scope, &file.path, &declaration.name);
    plan.created_symbols.push((
        symbol_id,
        Some(file_id),
        declaration.kind.clone(),
        declaration.name.clone(),
        scope,
    ));
    plan.symbol_edits.push(Operation::CreateSymbol {
        symbol_id,
        parent_id: Some(file_id),
        kind: declaration.kind.clone(),
        name: declaration.name.clone(),
        body: None,
        metadata: metadata_with_doc(
            json!({"declaration": declaration.declaration.as_deref().unwrap_or("")}),
            declaration.doc.as_deref(),
        ),
    });
    create_type_children(symbol_id, declaration, plan);
}

fn create_type_children(symbol_id: Uuid, declaration: &Declaration, plan: &mut Plan) {
    if declaration.kind == "struct" {
        for field in &declaration.fields {
            plan.symbol_edits.push(Operation::CreateSymbol {
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
    }
    if declaration.kind == "interface" {
        for method in &declaration.methods {
            plan.symbol_edits.push(Operation::CreateSymbol {
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
    }
}
