use super::{Plan, base::children_of_kind, matcher::match_by_name, queue_delete};
use crate::{
    import::{package_scope, value_id},
    model::ParsedFile,
};
use anyhow::Result;
use bonhomme_core::{Operation, SemanticGraph, SymbolNode, metadata_string};
use serde_json::json;
use uuid::Uuid;

pub(super) fn recover_file_metadata(
    base_file: &SymbolNode,
    edited_file: &ParsedFile,
    plan: &mut Plan,
) {
    let package_changed = metadata_string(&base_file.metadata, "package").as_deref()
        != Some(&edited_file.package_name);
    let imports_changed =
        metadata_string(&base_file.metadata, "imports").as_deref() != Some(&edited_file.imports);
    if package_changed || imports_changed {
        plan.symbol_edits.push(Operation::UpdateSymbol {
            symbol_id: base_file.id,
            name: None,
            body: None,
            metadata: Some(json!({
                "handler": "go",
                "path": edited_file.path,
                "package": edited_file.package_name,
                "imports": edited_file.imports,
            })),
        });
    }
}

pub(super) fn recover_top_level_values(
    base: &SemanticGraph,
    file_id: Uuid,
    file: &ParsedFile,
    plan: &mut Plan,
) -> Result<()> {
    let scope = package_scope(file);
    for kind in ["const", "var", "type"] {
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
            let declaration = edited_symbol.declaration.clone().unwrap_or_default();
            if base_symbol.name != edited_symbol.name || base_symbol.declaration != declaration {
                plan.symbol_edits.push(Operation::UpdateSymbol {
                    symbol_id: base_symbol.id,
                    name: (base_symbol.name != edited_symbol.name)
                        .then(|| edited_symbol.name.clone()),
                    body: None,
                    metadata: Some(json!({
                        "declaration": declaration,
                        "path": file.path,
                    })),
                });
            }
        }
        for edited_index in matches.added {
            let edited_symbol = edited[edited_index];
            plan.symbol_edits.push(Operation::CreateSymbol {
                symbol_id: value_id(&scope, &file.path, kind, &edited_symbol.name),
                parent_id: Some(file_id),
                kind: kind.to_string(),
                name: edited_symbol.name.clone(),
                body: None,
                metadata: json!({
                    "declaration": edited_symbol.declaration.as_deref().unwrap_or(""),
                    "path": file.path,
                }),
            });
        }
        for base_index in matches.deleted {
            queue_delete(base_symbols[base_index].id, plan);
        }
    }
    Ok(())
}
