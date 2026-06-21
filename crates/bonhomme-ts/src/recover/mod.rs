mod base;
mod matcher;
mod references;

use self::{
    base::{BaseClass, BaseFile, base_files_by_path},
    matcher::match_container,
    references::{ReferencePlan, SymbolIdentity, recover_reference_operations},
};
use crate::{
    oxc_parse::metadata_with_doc,
    parse::{ParsedClass, ParsedFile, ParsedFunction, ParsedMethod, parse_files},
    scanner::stable_import_uuid,
};
use anyhow::{Context, Result, bail};
use bonhomme_core::{Operation, RenderedFile, SemanticGraph};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

#[derive(Default)]
struct PlannedOperations {
    reference_deletes: Vec<Operation>,
    symbol_deletes: Vec<Operation>,
    symbol_edits: Vec<Operation>,
    reference_creates: Vec<Operation>,
    references: ReferencePlan,
}

pub fn recover_operations(
    base: &SemanticGraph,
    scope: &[Uuid],
    edited: &[RenderedFile],
) -> Result<Vec<Operation>> {
    let base_files = base_files_by_path(base, scope)?;
    let edited_files = parse_files(edited)?;
    let edited_sources = edited
        .iter()
        .map(|file| (file.path.clone(), file.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut planned = PlannedOperations::default();

    for (path, edited_file) in edited_files {
        let Some(base_file) = base_files.get(&path) else {
            let source = edited_sources
                .get(&path)
                .with_context(|| format!("edited source for {path} is missing"))?;
            planned
                .symbol_edits
                .extend(crate::import_typescript_files(std::slice::from_ref(
                    source,
                ))?);
            continue;
        };

        recover_file(base_file, &edited_file, &mut planned)?;
    }

    let (reference_deletes, reference_creates) =
        recover_reference_operations(base, &planned.references);
    planned.reference_deletes.extend(reference_deletes);
    planned.reference_creates.extend(reference_creates);

    let mut operations = planned.reference_deletes;
    operations.extend(planned.symbol_deletes);
    operations.extend(planned.symbol_edits);
    operations.extend(planned.reference_creates);
    Ok(operations)
}

fn recover_file(
    base_file: &BaseFile,
    edited_file: &ParsedFile,
    planned: &mut PlannedOperations,
) -> Result<()> {
    recover_functions(base_file, &edited_file.functions, planned)?;
    recover_classes(base_file, &edited_file.classes, planned)
}

fn recover_functions(
    base_file: &BaseFile,
    edited_functions: &[ParsedFunction],
    planned: &mut PlannedOperations,
) -> Result<()> {
    let matches = match_container(
        &base_file.functions,
        edited_functions,
        "function",
        &format!("file {}", base_file.path),
    )?;
    for (base_index, edited_index) in matches.matched {
        let base = &base_file.functions[base_index];
        let edited = &edited_functions[edited_index];
        let body_changed = base.body.trim() != edited.body.trim();
        if body_changed {
            planned
                .references
                .edited_calls
                .insert(base.id, edited.calls.clone());
        }
        if base.name != edited.name {
            planned
                .references
                .renamed_symbols
                .insert(base.id, edited.name.clone());
        }
        if base.name != edited.name
            || base.signature != edited.signature
            || body_changed
            || base.doc.as_deref() != edited.doc.as_deref()
        {
            planned.symbol_edits.push(Operation::UpdateSymbol {
                symbol_id: base.id,
                name: (base.name != edited.name).then(|| edited.name.clone()),
                body: Some(edited.body.clone()),
                metadata: Some(metadata_with_doc(
                    json!({
                        "declaration": edited.signature,
                        "exported": signature_is_exported(&edited.signature)
                    }),
                    edited.doc.as_deref(),
                )),
            });
        }
    }

    for edited_index in matches.added {
        let edited = &edited_functions[edited_index];
        let symbol_id = stable_import_uuid(&format!("function:{}:{}", base_file.path, edited.name));
        planned.references.created_symbols.push(SymbolIdentity {
            id: symbol_id,
            parent_id: Some(base_file.id),
            name: edited.name.clone(),
        });
        planned
            .references
            .edited_calls
            .insert(symbol_id, edited.calls.clone());
        planned.symbol_edits.push(Operation::CreateSymbol {
            symbol_id,
            parent_id: Some(base_file.id),
            kind: "function".to_string(),
            name: edited.name.clone(),
            body: Some(edited.body.clone()),
            metadata: metadata_with_doc(
                json!({
                    "declaration": edited.signature,
                    "exported": signature_is_exported(&edited.signature)
                }),
                edited.doc.as_deref(),
            ),
        });
    }

    for base_index in matches.deleted {
        let symbol_id = base_file.functions[base_index].id;
        planned.references.deleted_symbols.insert(symbol_id);
        planned
            .symbol_deletes
            .push(Operation::DeleteSymbol { symbol_id });
    }

    Ok(())
}

fn recover_classes(
    base_file: &BaseFile,
    edited_classes: &[ParsedClass],
    planned: &mut PlannedOperations,
) -> Result<()> {
    let mut consumed_classes = BTreeSet::new();
    for edited_class in edited_classes {
        let Some((base_index, base_class)) = base_file
            .classes
            .iter()
            .enumerate()
            .find(|(_, class)| class.name == edited_class.name)
        else {
            bail!(
                "structural recovery does not yet support new or renamed classes: {}",
                edited_class.name
            );
        };
        consumed_classes.insert(base_index);
        recover_methods(base_file, base_class, &edited_class.methods, planned)?;
    }

    for (index, base_class) in base_file.classes.iter().enumerate() {
        if !consumed_classes.contains(&index) {
            bail!(
                "structural recovery does not yet support class deletes: {}",
                base_class.name
            );
        }
    }

    Ok(())
}

fn recover_methods(
    base_file: &BaseFile,
    base_class: &BaseClass,
    edited_methods: &[ParsedMethod],
    planned: &mut PlannedOperations,
) -> Result<()> {
    let matches = match_container(
        &base_class.methods,
        edited_methods,
        "method",
        &format!("class {}", base_class.name),
    )?;
    for (base_index, edited_index) in matches.matched {
        let base = &base_class.methods[base_index];
        let edited = &edited_methods[edited_index];
        let body_changed = base.body.trim() != edited.body.trim();
        if body_changed {
            planned
                .references
                .edited_calls
                .insert(base.id, edited.calls.clone());
        }
        if base.name != edited.name {
            planned
                .references
                .renamed_symbols
                .insert(base.id, edited.name.clone());
        }
        if base.name != edited.name
            || base.signature != edited.signature
            || body_changed
            || base.doc.as_deref() != edited.doc.as_deref()
        {
            planned.symbol_edits.push(Operation::UpdateSymbol {
                symbol_id: base.id,
                name: (base.name != edited.name).then(|| edited.name.clone()),
                body: Some(edited.body.clone()),
                metadata: Some(metadata_with_doc(
                    json!({"signature": edited.signature}),
                    edited.doc.as_deref(),
                )),
            });
        }
    }

    for edited_index in matches.added {
        let edited = &edited_methods[edited_index];
        let symbol_id = stable_import_uuid(&format!(
            "method:{}:{}:{}",
            base_file.path, base_class.id, edited.name
        ));
        planned.references.created_symbols.push(SymbolIdentity {
            id: symbol_id,
            parent_id: Some(base_class.id),
            name: edited.name.clone(),
        });
        planned
            .references
            .edited_calls
            .insert(symbol_id, edited.calls.clone());
        planned.symbol_edits.push(Operation::CreateSymbol {
            symbol_id,
            parent_id: Some(base_class.id),
            kind: "method".to_string(),
            name: edited.name.clone(),
            body: Some(edited.body.clone()),
            metadata: metadata_with_doc(
                json!({"signature": edited.signature}),
                edited.doc.as_deref(),
            ),
        });
    }

    for base_index in matches.deleted {
        let symbol_id = base_class.methods[base_index].id;
        planned.references.deleted_symbols.insert(symbol_id);
        planned
            .symbol_deletes
            .push(Operation::DeleteSymbol { symbol_id });
    }

    Ok(())
}

fn signature_is_exported(signature: &str) -> bool {
    signature.split_whitespace().next() == Some("export")
}
