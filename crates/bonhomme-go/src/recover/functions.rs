use super::{
    Plan,
    base::{base_type_by_name, children_of_kind},
    matcher::match_by_body,
    queue_delete,
};
use crate::{
    import::{function_id, method_id},
    model::{Declaration, ParsedFile},
};
use anyhow::{Result, bail};
use bonhomme_core::{Operation, SemanticGraph};
use serde_json::json;
use std::collections::BTreeSet;
use uuid::Uuid;

pub(super) fn recover_functions(
    base: &SemanticGraph,
    file_id: Uuid,
    file: &ParsedFile,
    plan: &mut Plan,
) -> Result<()> {
    let base_functions = children_of_kind(base, file_id, "function");
    let edited = file
        .declarations
        .iter()
        .filter(|declaration| declaration.kind == "function")
        .collect::<Vec<_>>();
    let matches = match_by_body(
        &base_functions,
        &edited,
        &format!("Go functions in {}", file.path),
    )?;

    for (base_index, edited_index) in matches.matched {
        update_function_if_needed(
            &base_functions[base_index],
            edited[edited_index],
            file,
            plan,
        );
    }
    for edited_index in matches.added {
        create_function(file_id, edited[edited_index], file, plan);
    }
    for base_index in matches.deleted {
        queue_delete(base_functions[base_index].id, plan);
    }
    Ok(())
}

pub(super) fn recover_methods(
    base: &SemanticGraph,
    files: &[ParsedFile],
    plan: &mut Plan,
) -> Result<()> {
    let edited_methods = files
        .iter()
        .flat_map(|file| {
            file.declarations
                .iter()
                .filter(|declaration| declaration.kind == "method")
                .map(move |declaration| (file, declaration))
        })
        .collect::<Vec<_>>();

    for receiver in edited_receivers(&edited_methods) {
        let Some(type_symbol) = base_type_by_name(base, &receiver) else {
            bail!("Go receiver type {receiver} does not exist");
        };
        let base_methods = children_of_kind(base, type_symbol.id, "method")
            .into_iter()
            .filter(|method| !method.body.is_empty())
            .collect::<Vec<_>>();
        let edited = edited_methods
            .iter()
            .filter(|(_, declaration)| declaration.receiver.as_deref() == Some(&receiver))
            .copied()
            .collect::<Vec<_>>();
        let edited_decls = edited
            .iter()
            .map(|(_, declaration)| *declaration)
            .collect::<Vec<_>>();
        let matches = match_by_body(
            &base_methods,
            &edited_decls,
            &format!("Go methods on {receiver}"),
        )?;

        for (base_index, edited_index) in matches.matched {
            let (file, edited_method) = edited[edited_index];
            update_method_if_needed(
                &base_methods[base_index],
                edited_method,
                file,
                &receiver,
                plan,
            );
        }
        for edited_index in matches.added {
            let (file, edited_method) = edited[edited_index];
            create_method(type_symbol.id, edited_method, file, &receiver, plan);
        }
        for base_index in matches.deleted {
            queue_delete(base_methods[base_index].id, plan);
        }
    }

    Ok(())
}

fn update_function_if_needed(
    base_function: &super::base::BaseSymbol,
    edited_function: &Declaration,
    file: &ParsedFile,
    plan: &mut Plan,
) {
    let body = edited_function.body.clone().unwrap_or_default();
    let signature = edited_function.signature.clone().unwrap_or_default();
    if base_function.name != edited_function.name
        || base_function.signature != signature
        || base_function.body.trim() != body.trim()
    {
        plan.edited_calls
            .insert(base_function.id, edited_function.calls.clone());
        plan.symbol_edits.push(Operation::UpdateSymbol {
            symbol_id: base_function.id,
            name: (base_function.name != edited_function.name)
                .then(|| edited_function.name.clone()),
            body: Some(body),
            metadata: Some(json!({
                "signature": signature,
                "path": file.path,
            })),
        });
    }
}

fn create_function(
    file_id: Uuid,
    edited_function: &Declaration,
    file: &ParsedFile,
    plan: &mut Plan,
) {
    let symbol_id = function_id(&edited_function.name);
    plan.edited_calls
        .insert(symbol_id, edited_function.calls.clone());
    plan.created_symbols.push((
        symbol_id,
        Some(file_id),
        "function".to_string(),
        edited_function.name.clone(),
    ));
    plan.symbol_edits.push(Operation::CreateSymbol {
        symbol_id,
        parent_id: Some(file_id),
        kind: "function".to_string(),
        name: edited_function.name.clone(),
        body: edited_function.body.clone(),
        metadata: json!({
            "signature": edited_function.signature.as_deref().unwrap_or(""),
            "path": file.path,
        }),
    });
}

fn update_method_if_needed(
    base_method: &super::base::BaseSymbol,
    edited_method: &Declaration,
    file: &ParsedFile,
    receiver: &str,
    plan: &mut Plan,
) {
    let body = edited_method.body.clone().unwrap_or_default();
    let signature = edited_method.signature.clone().unwrap_or_default();
    if base_method.name != edited_method.name
        || base_method.signature != signature
        || base_method.body.trim() != body.trim()
    {
        plan.edited_calls
            .insert(base_method.id, edited_method.calls.clone());
        plan.symbol_edits.push(Operation::UpdateSymbol {
            symbol_id: base_method.id,
            name: (base_method.name != edited_method.name).then(|| edited_method.name.clone()),
            body: Some(body),
            metadata: Some(json!({
                "signature": signature,
                "receiver": receiver,
                "path": file.path,
            })),
        });
    }
}

fn create_method(
    type_symbol_id: Uuid,
    edited_method: &Declaration,
    file: &ParsedFile,
    receiver: &str,
    plan: &mut Plan,
) {
    let symbol_id = method_id(receiver, &edited_method.name);
    plan.edited_calls
        .insert(symbol_id, edited_method.calls.clone());
    plan.created_symbols.push((
        symbol_id,
        Some(type_symbol_id),
        "method".to_string(),
        edited_method.name.clone(),
    ));
    plan.symbol_edits.push(Operation::CreateSymbol {
        symbol_id,
        parent_id: Some(type_symbol_id),
        kind: "method".to_string(),
        name: edited_method.name.clone(),
        body: edited_method.body.clone(),
        metadata: json!({
            "signature": edited_method.signature.as_deref().unwrap_or(""),
            "receiver": receiver,
            "path": file.path,
        }),
    });
}

fn edited_receivers(edited_methods: &[(&ParsedFile, &Declaration)]) -> BTreeSet<String> {
    edited_methods
        .iter()
        .filter_map(|(_, declaration)| declaration.receiver.clone())
        .collect()
}
