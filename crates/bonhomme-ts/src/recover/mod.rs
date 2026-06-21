mod base;
mod matcher;
mod references;

use self::{
    base::{BaseClass, BaseFile, BaseFunction, BaseMethod, BaseProperty, base_files_by_path},
    matcher::match_container,
    references::{ReferencePlan, SymbolIdentity, recover_reference_operations},
};
use crate::{
    import::{
        class_metadata, file_metadata, function_metadata, method_metadata, property_metadata,
        resolved_class_id, resolved_file_id, resolved_function_id, resolved_method_id,
        resolved_property_id,
    },
    parse::{
        ParsedClass, ParsedClassMemberIndex, ParsedFile, ParsedFunction, ParsedMethod,
        ParsedProperty, ParsedTopLevelIndex, parse_files,
    },
};
use anyhow::{Result, bail};
use bonhomme_core::{Operation, RenderedFile, SemanticGraph};
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
    let edited_files = parse_files(edited)?;
    ensure_unique_symbol_ids(&edited_files)?;
    diff_graph_parsed_files(base, scope, edited_files)
}

pub(crate) fn diff_graph_parsed_files(
    base: &SemanticGraph,
    scope: &[Uuid],
    edited_files: BTreeMap<String, ParsedFile>,
) -> Result<Vec<Operation>> {
    let base_files = base_files_by_path(base, scope)?;
    diff_parsed_files(base, &base_files, edited_files)
}

fn diff_parsed_files(
    base: &SemanticGraph,
    base_files: &BTreeMap<String, BaseFile>,
    edited_files: BTreeMap<String, ParsedFile>,
) -> Result<Vec<Operation>> {
    let mut planned = PlannedOperations::default();

    for (path, edited_file) in edited_files {
        match base_files.get(&path) {
            Some(base_file) => recover_file(base_file, &edited_file, &mut planned)?,
            None => create_file(&edited_file, &mut planned),
        }
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

pub(crate) fn ensure_unique_symbol_ids(files: &BTreeMap<String, ParsedFile>) -> Result<()> {
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

fn recover_file(
    base_file: &BaseFile,
    edited_file: &ParsedFile,
    planned: &mut PlannedOperations,
) -> Result<()> {
    if base_file.preamble != edited_file.preamble {
        planned.symbol_edits.push(Operation::UpdateSymbol {
            symbol_id: base_file.id,
            name: None,
            body: None,
            metadata: Some(file_metadata(edited_file)),
        });
    }

    recover_functions(base_file, &edited_file.functions, planned)?;
    recover_classes(base_file, edited_file, planned)
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
        update_function(base, edited, planned);
    }

    for edited_index in matches.added {
        create_function(
            base_file.path.as_str(),
            base_file.id,
            &edited_functions[edited_index],
            planned,
        );
    }

    for base_index in matches.deleted {
        delete_function(&base_file.functions[base_index], planned);
    }

    Ok(())
}

fn update_function(base: &BaseFunction, edited: &ParsedFunction, planned: &mut PlannedOperations) {
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
            metadata: Some(function_metadata(edited)),
        });
    }
}

fn create_function(
    path: &str,
    file_id: Uuid,
    edited: &ParsedFunction,
    planned: &mut PlannedOperations,
) {
    let file = parsed_file_stub(path);
    let symbol_id = resolved_function_id(&file, edited);
    planned.references.created_symbols.push(SymbolIdentity {
        id: symbol_id,
        parent_id: Some(file_id),
        name: edited.name.clone(),
    });
    planned
        .references
        .edited_calls
        .insert(symbol_id, edited.calls.clone());
    planned.symbol_edits.push(Operation::CreateSymbol {
        symbol_id,
        parent_id: Some(file_id),
        kind: "function".to_string(),
        name: edited.name.clone(),
        body: Some(edited.body.clone()),
        metadata: function_metadata(edited),
    });
}

fn delete_function(base: &BaseFunction, planned: &mut PlannedOperations) {
    planned.references.deleted_symbols.insert(base.id);
    planned
        .symbol_deletes
        .push(Operation::DeleteSymbol { symbol_id: base.id });
}

fn recover_classes(
    base_file: &BaseFile,
    edited_file: &ParsedFile,
    planned: &mut PlannedOperations,
) -> Result<()> {
    let mut consumed_classes = BTreeSet::new();
    let base_by_id = base_file
        .classes
        .iter()
        .enumerate()
        .map(|(index, class)| (class.id, index))
        .collect::<BTreeMap<_, _>>();

    for edited_class in &edited_file.classes {
        let matched_index = edited_class
            .symbol_id
            .and_then(|id| base_by_id.get(&id).copied())
            .or_else(|| {
                base_file
                    .classes
                    .iter()
                    .enumerate()
                    .find(|(index, class)| {
                        !consumed_classes.contains(index) && class.name == edited_class.name
                    })
                    .map(|(index, _)| index)
            });

        match matched_index {
            Some(base_index) => {
                consumed_classes.insert(base_index);
                let base_class = &base_file.classes[base_index];
                update_class(base_class, edited_class, planned);
                recover_methods(base_file, base_class, &edited_class.methods, planned)?;
                recover_properties(base_file, base_class, &edited_class.properties, planned);
            }
            None => create_class(edited_file, base_file.id, edited_class, planned),
        }
    }

    for (index, base_class) in base_file.classes.iter().enumerate() {
        if !consumed_classes.contains(&index) {
            delete_class(base_class, planned);
        }
    }

    Ok(())
}

fn update_class(base: &BaseClass, edited: &ParsedClass, planned: &mut PlannedOperations) {
    if base.name != edited.name
        || base.declaration != edited.declaration
        || base.doc.as_deref() != edited.doc.as_deref()
    {
        planned.symbol_edits.push(Operation::UpdateSymbol {
            symbol_id: base.id,
            name: (base.name != edited.name).then(|| edited.name.clone()),
            body: None,
            metadata: Some(class_metadata(edited)),
        });
    }
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
        update_method(base, edited, planned);
    }

    for edited_index in matches.added {
        create_method(
            base_file.path.as_str(),
            base_class.id,
            &edited_methods[edited_index],
            planned,
        );
    }

    for base_index in matches.deleted {
        delete_method(&base_class.methods[base_index], planned);
    }

    Ok(())
}

fn update_method(base: &BaseMethod, edited: &ParsedMethod, planned: &mut PlannedOperations) {
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
        || base.kind != edited.kind
        || base.method_kind != edited.method_kind
        || base.is_static != edited.is_static
        || body_changed
        || base.doc.as_deref() != edited.doc.as_deref()
    {
        planned.symbol_edits.push(Operation::UpdateSymbol {
            symbol_id: base.id,
            name: (base.name != edited.name).then(|| edited.name.clone()),
            body: Some(edited.body.clone()),
            metadata: Some(method_metadata(edited)),
        });
    }
}

fn create_method(
    path: &str,
    class_id: Uuid,
    edited: &ParsedMethod,
    planned: &mut PlannedOperations,
) {
    let file = parsed_file_stub(path);
    let symbol_id = resolved_method_id(&file, class_id, edited);
    planned.references.created_symbols.push(SymbolIdentity {
        id: symbol_id,
        parent_id: Some(class_id),
        name: edited.name.clone(),
    });
    planned
        .references
        .edited_calls
        .insert(symbol_id, edited.calls.clone());
    planned.symbol_edits.push(Operation::CreateSymbol {
        symbol_id,
        parent_id: Some(class_id),
        kind: edited.kind.clone(),
        name: edited.name.clone(),
        body: Some(edited.body.clone()),
        metadata: method_metadata(edited),
    });
}

fn delete_method(base: &BaseMethod, planned: &mut PlannedOperations) {
    planned.references.deleted_symbols.insert(base.id);
    planned
        .symbol_deletes
        .push(Operation::DeleteSymbol { symbol_id: base.id });
}

fn recover_properties(
    base_file: &BaseFile,
    base_class: &BaseClass,
    edited_properties: &[ParsedProperty],
    planned: &mut PlannedOperations,
) {
    let mut consumed = BTreeSet::new();
    let base_by_id = base_class
        .properties
        .iter()
        .enumerate()
        .map(|(index, property)| (property.id, index))
        .collect::<BTreeMap<_, _>>();

    for edited in edited_properties {
        let matched_index = edited
            .symbol_id
            .and_then(|id| base_by_id.get(&id).copied())
            .or_else(|| {
                base_class
                    .properties
                    .iter()
                    .enumerate()
                    .find(|(index, property)| {
                        !consumed.contains(index) && property.name == edited.name
                    })
                    .map(|(index, _)| index)
            });

        match matched_index {
            Some(base_index) => {
                consumed.insert(base_index);
                update_property(&base_class.properties[base_index], edited, planned);
            }
            None => create_property(base_file.path.as_str(), base_class.id, edited, planned),
        }
    }

    for (index, base) in base_class.properties.iter().enumerate() {
        if !consumed.contains(&index) {
            delete_property(base, planned);
        }
    }
}

fn update_property(base: &BaseProperty, edited: &ParsedProperty, planned: &mut PlannedOperations) {
    if base.name != edited.name
        || base.declaration != edited.declaration
        || base.doc.as_deref() != edited.doc.as_deref()
    {
        planned.symbol_edits.push(Operation::UpdateSymbol {
            symbol_id: base.id,
            name: (base.name != edited.name).then(|| edited.name.clone()),
            body: None,
            metadata: Some(property_metadata(edited)),
        });
    }
}

fn create_property(
    path: &str,
    class_id: Uuid,
    edited: &ParsedProperty,
    planned: &mut PlannedOperations,
) {
    let file = parsed_file_stub(path);
    let symbol_id = resolved_property_id(&file, class_id, edited);
    planned.references.created_symbols.push(SymbolIdentity {
        id: symbol_id,
        parent_id: Some(class_id),
        name: edited.name.clone(),
    });
    planned.symbol_edits.push(Operation::CreateSymbol {
        symbol_id,
        parent_id: Some(class_id),
        kind: "property".to_string(),
        name: edited.name.clone(),
        body: None,
        metadata: property_metadata(edited),
    });
}

fn delete_property(base: &BaseProperty, planned: &mut PlannedOperations) {
    planned.references.deleted_symbols.insert(base.id);
    planned
        .symbol_deletes
        .push(Operation::DeleteSymbol { symbol_id: base.id });
}

fn create_file(file: &ParsedFile, planned: &mut PlannedOperations) {
    let file_id = resolved_file_id(file);
    planned.symbol_edits.push(Operation::CreateSymbol {
        symbol_id: file_id,
        parent_id: None,
        kind: "file".to_string(),
        name: file.path.clone(),
        body: None,
        metadata: file_metadata(file),
    });

    for entry in &file.top_level_order {
        match *entry {
            ParsedTopLevelIndex::Class(index) => {
                create_class(file, file_id, &file.classes[index], planned);
            }
            ParsedTopLevelIndex::Function(index) => {
                create_function(file.path.as_str(), file_id, &file.functions[index], planned);
            }
        }
    }
}

fn create_class(
    file: &ParsedFile,
    file_id: Uuid,
    class: &ParsedClass,
    planned: &mut PlannedOperations,
) {
    let class_id = resolved_class_id(file, class);
    planned.references.created_symbols.push(SymbolIdentity {
        id: class_id,
        parent_id: Some(file_id),
        name: class.name.clone(),
    });
    planned.symbol_edits.push(Operation::CreateSymbol {
        symbol_id: class_id,
        parent_id: Some(file_id),
        kind: "class".to_string(),
        name: class.name.clone(),
        body: None,
        metadata: class_metadata(class),
    });

    for member in &class.member_order {
        match *member {
            ParsedClassMemberIndex::Method(index) => {
                create_method(file.path.as_str(), class_id, &class.methods[index], planned);
            }
            ParsedClassMemberIndex::Property(index) => {
                create_property(
                    file.path.as_str(),
                    class_id,
                    &class.properties[index],
                    planned,
                );
            }
        }
    }
}

fn delete_class(base: &BaseClass, planned: &mut PlannedOperations) {
    for method in &base.methods {
        delete_method(method, planned);
    }
    for property in &base.properties {
        delete_property(property, planned);
    }
    planned.references.deleted_symbols.insert(base.id);
    planned
        .symbol_deletes
        .push(Operation::DeleteSymbol { symbol_id: base.id });
}

fn parsed_file_stub(path: &str) -> ParsedFile {
    ParsedFile {
        path: path.to_string(),
        file_symbol_id: None,
        preamble: String::new(),
        classes: Vec::new(),
        functions: Vec::new(),
        top_level_order: Vec::new(),
    }
}
