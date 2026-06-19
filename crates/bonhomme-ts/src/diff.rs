use crate::{
    import::import_typescript_files,
    parse::{ParsedFile, parse_files},
    scanner::stable_import_uuid,
};
use anyhow::{Context, Result, bail};
use bonhomme_core::{Operation, RenderedFile};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

pub fn diff_slice(original: &[RenderedFile], modified: &[RenderedFile]) -> Result<Vec<Operation>> {
    let original_by_path = parse_files(original)?;
    let modified_by_path = parse_files(modified)?;
    // A bonhomme:symbol id is a stable, unique identity. If the incoming slice claims the same id
    // for more than one symbol (e.g. an agent copy-pasted a member along with its comment), the
    // diff would silently emit conflicting UpdateSymbol ops on one id (last writer wins). Reject it
    // here, before any operation is generated, rather than corrupting identity downstream.
    ensure_unique_symbol_ids(&modified_by_path)?;
    let modified_sources = modified
        .iter()
        .map(|file| (file.path.clone(), file.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut operations = Vec::new();

    for (path, modified_file) in &modified_by_path {
        let Some(original_file) = original_by_path.get(path) else {
            let source = modified_sources
                .get(path)
                .with_context(|| format!("modified source for {path} is missing"))?;
            operations.extend(import_typescript_files(std::slice::from_ref(source))?);
            continue;
        };

        // Match modified symbols back to originals (by hidden id, then by name) so a comment-less
        // edit becomes an UpdateSymbol that preserves identity. `consumed` records every original
        // that was matched, so the delete pass only removes symbols that truly disappeared and we
        // never emit a DeleteSymbol + CreateSymbol pair that would dangle an inbound reference.
        let mut consumed = BTreeSet::new();
        let mut edits = Vec::new();
        edits.extend(diff_functions(original_file, modified_file, &mut consumed)?);
        edits.extend(diff_classes(
            path,
            original_file,
            modified_file,
            &mut consumed,
        )?);

        // Deletes are emitted before edits to preserve the historical operation ordering.
        operations.extend(diff_deletes(original_file, modified_file, &consumed)?);
        operations.extend(edits);
    }

    Ok(operations)
}

fn diff_functions(
    original_file: &ParsedFile,
    modified_file: &ParsedFile,
    consumed: &mut BTreeSet<Uuid>,
) -> Result<Vec<Operation>> {
    let mut operations = Vec::new();
    let original_by_id = original_file
        .functions
        .iter()
        .filter_map(|function| function.symbol_id.map(|id| (id, function)))
        .collect::<BTreeMap<_, _>>();

    for function in &modified_file.functions {
        let matched = function
            .symbol_id
            .and_then(|id| original_by_id.get(&id).copied())
            .or_else(|| {
                original_file.functions.iter().find(|original| {
                    original.symbol_id.is_some_and(|id| !consumed.contains(&id))
                        && original.name == function.name
                })
            });

        match matched {
            Some(original_function) => {
                let symbol_id = original_function
                    .symbol_id
                    .expect("matched original carries a symbol id");
                consumed.insert(symbol_id);
                if original_function.signature != function.signature
                    || original_function.body.trim() != function.body.trim()
                    || original_function.name != function.name
                {
                    operations.push(Operation::UpdateSymbol {
                        symbol_id,
                        name: (original_function.name != function.name)
                            .then(|| function.name.clone()),
                        body: Some(function.body.clone()),
                        metadata: Some(json!({
                            "declaration": function.signature,
                            "exported": signature_is_exported(&function.signature)
                        })),
                    });
                }
            }
            None => {
                let file_symbol_id = original_file.file_symbol_id.with_context(|| {
                    format!(
                        "file {} has no bonhomme symbol metadata",
                        original_file.path
                    )
                })?;
                operations.push(Operation::CreateSymbol {
                    symbol_id: stable_import_uuid(&format!(
                        "function:{}:{}",
                        original_file.path, function.name
                    )),
                    parent_id: Some(file_symbol_id),
                    kind: "function".to_string(),
                    name: function.name.clone(),
                    body: Some(function.body.clone()),
                    metadata: json!({
                        "declaration": function.signature,
                        "exported": signature_is_exported(&function.signature)
                    }),
                });
            }
        }
    }

    Ok(operations)
}

fn diff_classes(
    path: &str,
    original_file: &ParsedFile,
    modified_file: &ParsedFile,
    consumed: &mut BTreeSet<Uuid>,
) -> Result<Vec<Operation>> {
    let mut operations = Vec::new();
    let original_methods_by_id = original_file
        .classes
        .iter()
        .flat_map(|class| &class.methods)
        .filter_map(|method| method.symbol_id.map(|id| (id, method)))
        .collect::<BTreeMap<_, _>>();

    for modified_class in &modified_file.classes {
        let original_class = original_file
            .classes
            .iter()
            .find(|class| {
                class.symbol_id == modified_class.symbol_id || class.name == modified_class.name
            })
            .with_context(|| format!("class {} is new or missing metadata", modified_class.name))?;
        let parent_id = original_class.symbol_id.with_context(|| {
            format!(
                "class {} has no bonhomme symbol metadata",
                original_class.name
            )
        })?;

        for method in &modified_class.methods {
            let matched = method
                .symbol_id
                .and_then(|id| original_methods_by_id.get(&id).copied())
                .or_else(|| {
                    original_class.methods.iter().find(|original| {
                        original.symbol_id.is_some_and(|id| !consumed.contains(&id))
                            && original.name == method.name
                    })
                });

            match matched {
                Some(original_method) => {
                    let symbol_id = original_method
                        .symbol_id
                        .expect("matched original carries a symbol id");
                    consumed.insert(symbol_id);
                    if original_method.signature != method.signature
                        || original_method.body.trim() != method.body.trim()
                        || original_method.name != method.name
                    {
                        operations.push(Operation::UpdateSymbol {
                            symbol_id,
                            name: (original_method.name != method.name)
                                .then(|| method.name.clone()),
                            body: Some(method.body.clone()),
                            metadata: Some(json!({"signature": method.signature})),
                        });
                    }
                }
                None => {
                    operations.push(Operation::CreateSymbol {
                        symbol_id: stable_import_uuid(&format!(
                            "method:{path}:{parent_id}:{}",
                            method.name
                        )),
                        parent_id: Some(parent_id),
                        kind: "method".to_string(),
                        name: method.name.clone(),
                        body: Some(method.body.clone()),
                        metadata: json!({"signature": method.signature}),
                    });
                }
            }
        }
    }

    Ok(operations)
}

fn diff_deletes(
    original_file: &ParsedFile,
    modified_file: &ParsedFile,
    consumed: &BTreeSet<Uuid>,
) -> Result<Vec<Operation>> {
    let mut operations = Vec::new();

    for original_function in &original_file.functions {
        if let Some(symbol_id) = original_function.symbol_id
            && !consumed.contains(&symbol_id)
        {
            operations.push(Operation::DeleteSymbol { symbol_id });
        }
    }

    for original_class in &original_file.classes {
        let Some(_modified_class) = modified_file.classes.iter().find(|class| {
            class.symbol_id == original_class.symbol_id || class.name == original_class.name
        }) else {
            if original_class.symbol_id.is_some() {
                bail!(
                    "class deletes are not supported by the v1 slice diff prototype: {}",
                    original_class.name
                );
            }
            continue;
        };
        for original_method in &original_class.methods {
            if let Some(symbol_id) = original_method.symbol_id
                && !consumed.contains(&symbol_id)
            {
                operations.push(Operation::DeleteSymbol { symbol_id });
            }
        }
    }

    Ok(operations)
}

/// Whether a parsed top-level function signature carries the `export` keyword, tolerating any
/// whitespace (tab/newline) between `export` and `function` so it round-trips with the importer.
fn signature_is_exported(signature: &str) -> bool {
    signature.split_whitespace().next() == Some("export")
}

/// Reject a slice that reuses a `bonhomme:symbol` id for more than one symbol across the whole
/// edited set. Such an id is a stable, globally-unique identity, so a duplicate can only come from
/// a corrupted/duplicated identity comment and must not be turned into operations.
fn ensure_unique_symbol_ids(files: &BTreeMap<String, ParsedFile>) -> Result<()> {
    let mut seen = BTreeSet::new();
    for file in files.values() {
        for id in file.symbol_ids() {
            if !seen.insert(id) {
                bail!(
                    "slice reuses bonhomme:symbol id {id} for more than one symbol; \
                     identity comments must be unique"
                );
            }
        }
    }
    Ok(())
}
