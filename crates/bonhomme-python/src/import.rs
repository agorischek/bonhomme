use crate::{
    ids::{attribute_id, class_id, file_id, function_id, method_id, reference_id, value_id},
    model::{CallTarget, Declaration, Member, ParsedFile, ParsedProject, PythonMethod},
};
use anyhow::{Context, Result, bail};
use bonhomme_core::{Operation, RenderedFile};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use tree_sitter::{Node, Parser};
use uuid::Uuid;

const CALLS_KIND: &str = "calls";

#[derive(Default)]
pub(crate) struct ImportIndexes {
    classes_by_name: BTreeMap<String, Vec<Uuid>>,
    functions_by_name: BTreeMap<String, Vec<Uuid>>,
    methods_by_key: BTreeMap<(Uuid, String), Uuid>,
    methods_by_name: BTreeMap<String, Vec<Uuid>>,
    parent_by_symbol: BTreeMap<Uuid, Uuid>,
    calls: BTreeMap<Uuid, Vec<CallTarget>>,
}

pub fn import_python_files(files: &[RenderedFile]) -> Result<Vec<Operation>> {
    let parsed = parse_python_files(files)?;
    operations_from_parsed_project(&parsed)
}

pub(crate) fn parse_python_files(files: &[RenderedFile]) -> Result<ParsedProject> {
    let mut parsed = ParsedProject::default();
    for file in files {
        parsed.files.push(parse_file(&file.path, &file.content)?);
    }
    parsed
        .files
        .sort_by(|left, right| left.path.cmp(&right.path));
    Ok(parsed)
}

pub(crate) fn operations_from_parsed_project(parsed: &ParsedProject) -> Result<Vec<Operation>> {
    let mut indexes = ImportIndexes::default();
    index_project(parsed, &mut indexes);

    let mut operations = Vec::new();
    for file in &parsed.files {
        operations.push(file_operation(file));
    }
    for file in &parsed.files {
        operations.extend(declaration_operations(file, &mut indexes));
    }
    operations.extend(reference_operations(&indexes));
    Ok(operations)
}

fn parse_file(path: &str, source: &str) -> Result<ParsedFile> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .context("failed to load Python tree-sitter grammar")?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow::anyhow!("tree-sitter failed to parse Python source"))?;
    let root = tree.root_node();
    if root.has_error() {
        bail!("{path} is not valid Python");
    }

    let bytes = source.as_bytes();
    let mut declarations = Vec::new();
    let mut ranges = Vec::new();
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        if let Some(declaration) = parse_top_level(source, bytes, child)? {
            ranges.push((child.start_byte(), child.end_byte()));
            declarations.push(declaration);
        }
    }

    Ok(ParsedFile {
        path: path.to_string(),
        preamble: outside_ranges(source, &ranges),
        declarations,
    })
}

fn parse_top_level(source: &str, bytes: &[u8], node: Node<'_>) -> Result<Option<Declaration>> {
    match node.kind() {
        "function_definition" => parse_function(source, bytes, node, node.start_byte())
            .map(|function| Some(function_declaration(function))),
        "class_definition" => parse_class(source, bytes, node, node.start_byte()).map(Some),
        "decorated_definition" => parse_decorated_top_level(source, bytes, node),
        "expression_statement" => Ok(parse_value(bytes, node).map(value_declaration)),
        _ => Ok(None),
    }
}

fn parse_decorated_top_level(
    source: &str,
    bytes: &[u8],
    node: Node<'_>,
) -> Result<Option<Declaration>> {
    let Some(definition) = node.child_by_field_name("definition") else {
        return Ok(None);
    };
    match definition.kind() {
        "function_definition" => parse_function(source, bytes, definition, node.start_byte())
            .map(|function| Some(function_declaration(function))),
        "class_definition" => parse_class(source, bytes, definition, node.start_byte()).map(Some),
        _ => Ok(None),
    }
}

fn parse_class(
    source: &str,
    bytes: &[u8],
    node: Node<'_>,
    declaration_start: usize,
) -> Result<Declaration> {
    let name = field_text(node, "name", bytes).context("Python class missing name")?;
    let body = node
        .child_by_field_name("body")
        .context("Python class missing body")?;
    let mut methods = Vec::new();
    let mut attributes = Vec::new();
    let mut ranges = Vec::new();
    let mut cursor = body.walk();
    for child in body.children(&mut cursor) {
        match child.kind() {
            "function_definition" => {
                methods.push(parse_function(source, bytes, child, child.start_byte())?);
                ranges.push((child.start_byte(), child.end_byte()));
            }
            "decorated_definition" => {
                if let Some(definition) = child.child_by_field_name("definition")
                    && definition.kind() == "function_definition"
                {
                    methods.push(parse_function(
                        source,
                        bytes,
                        definition,
                        child.start_byte(),
                    )?);
                    ranges.push((child.start_byte(), child.end_byte()));
                }
            }
            "expression_statement" => {
                if let Some(attribute) = parse_value(bytes, child) {
                    attributes.push(attribute);
                    ranges.push((child.start_byte(), child.end_byte()));
                }
            }
            _ => {}
        }
    }

    Ok(Declaration {
        kind: "class".to_string(),
        name,
        signature: Some(header_without_colon(
            source,
            declaration_start,
            body.start_byte(),
        )),
        preamble: Some(outside_subranges(
            source,
            body.start_byte(),
            body.end_byte(),
            &ranges,
        )),
        methods,
        attributes,
        ..Declaration::default()
    })
}

fn parse_function(
    source: &str,
    bytes: &[u8],
    node: Node<'_>,
    declaration_start: usize,
) -> Result<PythonMethod> {
    let name = field_text(node, "name", bytes).context("Python function missing name")?;
    let body = node
        .child_by_field_name("body")
        .context("Python function missing body")?;
    Ok(PythonMethod {
        name,
        signature: header_without_colon(source, declaration_start, body.start_byte()),
        body: dedent_block(source, body),
        calls: collect_calls(body, bytes),
    })
}

fn function_declaration(function: PythonMethod) -> Declaration {
    Declaration {
        kind: "function".to_string(),
        name: function.name,
        signature: Some(function.signature),
        body: Some(function.body),
        calls: function.calls,
        ..Declaration::default()
    }
}

fn parse_value(bytes: &[u8], node: Node<'_>) -> Option<Member> {
    let child = node.named_child(0)?;
    if !matches!(
        child.kind(),
        "assignment" | "augmented_assignment" | "type_alias_statement"
    ) {
        return None;
    }
    let left = child
        .child_by_field_name("left")
        .or_else(|| child.child_by_field_name("name"))?;
    let name = simple_identifier(left, bytes)?;
    Some(Member {
        name,
        declaration: node_text(node, bytes)?.trim().to_string(),
    })
}

fn value_declaration(value: Member) -> Declaration {
    Declaration {
        kind: "value".to_string(),
        name: value.name,
        declaration: Some(value.declaration),
        ..Declaration::default()
    }
}

fn index_project(parsed: &ParsedProject, indexes: &mut ImportIndexes) {
    for file in &parsed.files {
        for declaration in &file.declarations {
            match declaration.kind.as_str() {
                "class" => {
                    let symbol_id = class_id(&file.path, &declaration.name);
                    indexes
                        .classes_by_name
                        .entry(declaration.name.clone())
                        .or_default()
                        .push(symbol_id);
                    for method in &declaration.methods {
                        let method_id = method_id(symbol_id, &method.name);
                        indexes
                            .methods_by_key
                            .insert((symbol_id, method.name.clone()), method_id);
                        indexes
                            .methods_by_name
                            .entry(method.name.clone())
                            .or_default()
                            .push(method_id);
                        indexes.parent_by_symbol.insert(method_id, symbol_id);
                        indexes.calls.insert(method_id, method.calls.clone());
                    }
                }
                "function" => {
                    let symbol_id = function_id(&file.path, &declaration.name);
                    indexes
                        .functions_by_name
                        .entry(declaration.name.clone())
                        .or_default()
                        .push(symbol_id);
                    indexes.calls.insert(symbol_id, declaration.calls.clone());
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
            "handler": "python",
            "path": file.path,
            "preamble": file.preamble,
        }),
    }
}

fn declaration_operations(file: &ParsedFile, indexes: &mut ImportIndexes) -> Vec<Operation> {
    let mut operations = Vec::new();
    let file_id = file_id(&file.path);
    for declaration in &file.declarations {
        match declaration.kind.as_str() {
            "class" => operations.extend(class_operations(file, file_id, declaration, indexes)),
            "function" => operations.push(function_operation(file, file_id, declaration)),
            "value" => operations.push(value_operation(file, file_id, declaration)),
            _ => {}
        }
    }
    operations
}

fn class_operations(
    file: &ParsedFile,
    file_id: Uuid,
    declaration: &Declaration,
    indexes: &mut ImportIndexes,
) -> Vec<Operation> {
    let symbol_id = class_id(&file.path, &declaration.name);
    let mut operations = vec![Operation::CreateSymbol {
        symbol_id,
        parent_id: Some(file_id),
        kind: "class".to_string(),
        name: declaration.name.clone(),
        body: None,
        metadata: json!({
            "signature": declaration.signature.as_deref().unwrap_or(""),
            "bodyPreamble": declaration.preamble.as_deref().unwrap_or(""),
            "path": file.path,
        }),
    }];
    for attribute in &declaration.attributes {
        operations.push(Operation::CreateSymbol {
            symbol_id: attribute_id(symbol_id, &attribute.name),
            parent_id: Some(symbol_id),
            kind: "attribute".to_string(),
            name: attribute.name.clone(),
            body: None,
            metadata: json!({
                "declaration": attribute.declaration,
                "path": file.path,
            }),
        });
    }
    for method in &declaration.methods {
        operations.push(method_operation(file, symbol_id, method, indexes));
    }
    operations
}

fn function_operation(file: &ParsedFile, file_id: Uuid, declaration: &Declaration) -> Operation {
    Operation::CreateSymbol {
        symbol_id: function_id(&file.path, &declaration.name),
        parent_id: Some(file_id),
        kind: "function".to_string(),
        name: declaration.name.clone(),
        body: declaration.body.clone(),
        metadata: json!({
            "signature": declaration.signature.as_deref().unwrap_or(""),
            "path": file.path,
        }),
    }
}

fn value_operation(file: &ParsedFile, file_id: Uuid, declaration: &Declaration) -> Operation {
    Operation::CreateSymbol {
        symbol_id: value_id(&file.path, &declaration.name),
        parent_id: Some(file_id),
        kind: "value".to_string(),
        name: declaration.name.clone(),
        body: None,
        metadata: json!({
            "declaration": declaration.declaration.as_deref().unwrap_or(""),
            "path": file.path,
        }),
    }
}

fn method_operation(
    file: &ParsedFile,
    parent_id: Uuid,
    method: &PythonMethod,
    indexes: &mut ImportIndexes,
) -> Operation {
    let symbol_id = method_id(parent_id, &method.name);
    indexes.parent_by_symbol.insert(symbol_id, parent_id);
    indexes.calls.insert(symbol_id, method.calls.clone());
    Operation::CreateSymbol {
        symbol_id,
        parent_id: Some(parent_id),
        kind: "method".to_string(),
        name: method.name.clone(),
        body: Some(method.body.clone()),
        metadata: json!({
            "signature": method.signature,
            "path": file.path,
        }),
    }
}

pub(crate) fn reference_operations(indexes: &ImportIndexes) -> Vec<Operation> {
    let mut seen = BTreeSet::new();
    let mut operations = Vec::new();
    for (from_symbol_id, calls) in &indexes.calls {
        for call in calls {
            let Some(to_symbol_id) = resolve_call(indexes, *from_symbol_id, call) else {
                continue;
            };
            if to_symbol_id == *from_symbol_id
                || !seen.insert((*from_symbol_id, to_symbol_id, CALLS_KIND))
            {
                continue;
            }
            operations.push(Operation::CreateReference {
                reference_id: reference_id(*from_symbol_id, to_symbol_id, CALLS_KIND),
                from_symbol_id: *from_symbol_id,
                to_symbol_id,
                kind: CALLS_KIND.to_string(),
            });
        }
    }
    operations
}

fn resolve_call(indexes: &ImportIndexes, from_symbol_id: Uuid, call: &CallTarget) -> Option<Uuid> {
    match call {
        CallTarget::Free(name) => unique(indexes.functions_by_name.get(name)?)
            .or_else(|| unique(indexes.classes_by_name.get(name)?)),
        CallTarget::This(name) => {
            let parent_id = indexes.parent_by_symbol.get(&from_symbol_id)?;
            indexes
                .methods_by_key
                .get(&(*parent_id, name.clone()))
                .copied()
        }
        CallTarget::Method(name) => unique(indexes.methods_by_name.get(name)?),
    }
}

fn unique(ids: &[Uuid]) -> Option<Uuid> {
    (ids.len() == 1).then_some(ids[0])
}

fn collect_calls(node: Node<'_>, bytes: &[u8]) -> Vec<CallTarget> {
    let mut calls = Vec::new();
    collect_calls_inner(node, bytes, &mut calls);
    calls.sort();
    calls.dedup();
    calls
}

fn collect_calls_inner(node: Node<'_>, bytes: &[u8], calls: &mut Vec<CallTarget>) {
    if node.kind() == "call"
        && let Some(function) = node.child_by_field_name("function")
        && let Some(call) = call_target(function, bytes)
    {
        calls.push(call);
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_calls_inner(child, bytes, calls);
    }
}

fn call_target(node: Node<'_>, bytes: &[u8]) -> Option<CallTarget> {
    match node.kind() {
        "identifier" => Some(CallTarget::Free(node_text(node, bytes)?)),
        "attribute" => {
            let attribute = field_text(node, "attribute", bytes)?;
            let object = node.child_by_field_name("object")?;
            if object.kind() == "identifier"
                && matches!(node_text(object, bytes)?.as_str(), "self" | "cls")
            {
                Some(CallTarget::This(attribute))
            } else {
                Some(CallTarget::Method(attribute))
            }
        }
        _ => None,
    }
}

fn simple_identifier(node: Node<'_>, bytes: &[u8]) -> Option<String> {
    match node.kind() {
        "identifier" => node_text(node, bytes),
        "pattern_list" | "tuple_pattern" | "list_pattern" => {
            let first = node.named_child(0)?;
            simple_identifier(first, bytes)
        }
        _ => None,
    }
}

fn field_text(node: Node<'_>, field: &str, bytes: &[u8]) -> Option<String> {
    node_text(node.child_by_field_name(field)?, bytes)
}

fn node_text(node: Node<'_>, bytes: &[u8]) -> Option<String> {
    node.utf8_text(bytes).ok().map(ToString::to_string)
}

fn header_without_colon(source: &str, start: usize, body_start: usize) -> String {
    let mut header = source[start..body_start].trim_end().to_string();
    while header.ends_with(':') {
        header.pop();
        header = header.trim_end().to_string();
    }
    header
}

fn dedent_block(source: &str, node: Node<'_>) -> String {
    let Some(text) = node_text(node, source.as_bytes()) else {
        return String::new();
    };
    dedent_text(&text)
}

fn dedent_text(text: &str) -> String {
    let mut lines = text.lines().collect::<Vec<_>>();
    while lines.first().is_some_and(|line| line.trim().is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    let indent = lines
        .iter()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            line.chars()
                .take_while(|ch| *ch == ' ' || *ch == '\t')
                .count()
        })
        .min()
        .unwrap_or(0);
    lines
        .into_iter()
        .map(|line| {
            if line.len() >= indent {
                line[indent..].trim_end().to_string()
            } else {
                line.trim_end().to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn outside_ranges(source: &str, ranges: &[(usize, usize)]) -> String {
    outside_subranges(source, 0, source.len(), ranges)
}

fn outside_subranges(source: &str, start: usize, end: usize, ranges: &[(usize, usize)]) -> String {
    let mut sorted = ranges
        .iter()
        .copied()
        .filter(|(range_start, range_end)| *range_start >= start && *range_end <= end)
        .collect::<Vec<_>>();
    sorted.sort_by_key(|range| range.0);

    let mut out = String::new();
    let mut cursor = start;
    for (range_start, range_end) in sorted {
        if cursor < range_start {
            out.push_str(&source[cursor..range_start]);
        }
        cursor = cursor.max(range_end);
    }
    if cursor < end {
        out.push_str(&source[cursor..end]);
    }
    dedent_text(&out)
}
