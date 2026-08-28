use crate::{
    ids::{file_id, member_id, reference_id, type_id},
    model::{
        CallTarget, FileDeclaration, MemberDeclaration, ParsedFile, ParsedProject, TypeDeclaration,
        TypeMember,
    },
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
    types_by_name: BTreeMap<String, Vec<Uuid>>,
    functions_by_name: BTreeMap<String, Vec<Uuid>>,
    members_by_key: BTreeMap<(Uuid, String), Vec<Uuid>>,
    members_by_name: BTreeMap<String, Vec<Uuid>>,
    parent_by_symbol: BTreeMap<Uuid, Uuid>,
    calls: BTreeMap<Uuid, Vec<CallTarget>>,
}

pub fn import_kotlin_files(files: &[RenderedFile]) -> Result<Vec<Operation>> {
    let parsed = parse_kotlin_files(files)?;
    operations_from_parsed_project(&parsed)
}

pub(crate) fn parse_kotlin_files(files: &[RenderedFile]) -> Result<ParsedProject> {
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
        operations.extend(declaration_operations(file));
    }
    operations.extend(reference_operations(&indexes));
    Ok(operations)
}

fn parse_file(path: &str, source: &str) -> Result<ParsedFile> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_kotlin_ng::LANGUAGE.into())
        .context("failed to load Kotlin tree-sitter grammar")?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow::anyhow!("tree-sitter failed to parse Kotlin source"))?;
    let root = tree.root_node();
    if root.has_error() {
        bail!("{path} is not valid Kotlin");
    }

    let bytes = source.as_bytes();
    let mut file = ParsedFile {
        path: path.to_string(),
        ..ParsedFile::default()
    };
    let mut ranges = Vec::new();
    let mut leading_cursor = 0;
    let mut cursor = root.walk();
    for raw_child in root.children(&mut cursor) {
        if !raw_child.is_named() {
            continue;
        }
        let child = unwrap_declaration(raw_child);
        match child.kind() {
            "package_header" => {
                file.package = node_text(child, bytes)
                    .map(|text| text.trim().trim_start_matches("package").trim().to_string());
                leading_cursor = child.end_byte();
            }
            "import" => {
                if let Some(import) = node_text(child, bytes) {
                    file.imports.push(import.trim().to_string());
                }
                leading_cursor = child.end_byte();
            }
            kind if is_type_kind(kind) || is_member_kind(kind) => {
                let declaration_start = leading_cursor.min(raw_child.start_byte());
                if let Some(declaration) =
                    parse_file_declaration(source, bytes, child, declaration_start)?
                {
                    ranges.push((declaration_start, raw_child.end_byte()));
                    file.declarations.push(declaration);
                    leading_cursor = raw_child.end_byte();
                }
            }
            _ => {}
        }
    }
    file.preamble = outside_ranges(source, &ranges);
    Ok(file)
}

fn parse_file_declaration(
    source: &str,
    bytes: &[u8],
    node: Node<'_>,
    declaration_start: usize,
) -> Result<Option<FileDeclaration>> {
    match node.kind() {
        kind if is_type_kind(kind) => Ok(Some(FileDeclaration::Type(parse_type(
            source,
            bytes,
            node,
            declaration_start,
        )?))),
        kind if is_member_kind(kind) => {
            Ok(parse_member(source, bytes, node, declaration_start)?.map(FileDeclaration::Member))
        }
        _ => Ok(None),
    }
}

fn parse_type(
    source: &str,
    bytes: &[u8],
    node: Node<'_>,
    declaration_start: usize,
) -> Result<TypeDeclaration> {
    let name = type_name(node, bytes).context("Kotlin type declaration missing name")?;
    let body = type_body(node);
    let mut members = Vec::new();
    let mut ranges = Vec::new();
    if let Some(body) = body {
        let (inner_start, inner_end) = inner_brace_bounds(source, body);
        parse_type_body(
            source,
            bytes,
            body,
            inner_start,
            inner_end,
            &mut members,
            &mut ranges,
        )?;
    }

    let signature = body
        .map(|body| dedent_text(&header_before(source, declaration_start, body.start_byte())))
        .unwrap_or_else(|| dedent_text(source[declaration_start..node.end_byte()].trim()));
    let body_preamble = body
        .map(|body| {
            let (start, end) = inner_brace_bounds(source, body);
            outside_subranges(source, start, end, &ranges)
        })
        .unwrap_or_default();

    Ok(TypeDeclaration {
        kind: type_kind(node, source).to_string(),
        name,
        signature,
        body_preamble,
        members,
    })
}

fn parse_type_body(
    source: &str,
    bytes: &[u8],
    body: Node<'_>,
    start: usize,
    end: usize,
    members: &mut Vec<TypeMember>,
    ranges: &mut Vec<(usize, usize)>,
) -> Result<()> {
    let mut leading_cursor = start;
    let mut cursor = body.walk();
    for raw_child in body.children(&mut cursor) {
        if !raw_child.is_named() || raw_child.end_byte() <= start || raw_child.start_byte() >= end {
            continue;
        }
        let child = unwrap_declaration(raw_child);
        let declaration_start = leading_cursor.min(raw_child.start_byte());
        if is_type_kind(child.kind()) {
            members.push(TypeMember::Type(parse_type(
                source,
                bytes,
                child,
                declaration_start,
            )?));
            ranges.push((declaration_start, raw_child.end_byte()));
            leading_cursor = raw_child.end_byte();
        } else if let Some(member) = parse_member(source, bytes, child, declaration_start)? {
            members.push(TypeMember::Member(member));
            ranges.push((declaration_start, raw_child.end_byte()));
            leading_cursor = raw_child.end_byte();
        }
    }
    Ok(())
}

fn parse_member(
    source: &str,
    bytes: &[u8],
    node: Node<'_>,
    declaration_start: usize,
) -> Result<Option<MemberDeclaration>> {
    match node.kind() {
        "function_declaration" => {
            parse_function(source, bytes, node, declaration_start, "function").map(Some)
        }
        "property_declaration" => Ok(parse_property(source, bytes, node, declaration_start)),
        "secondary_constructor" => Ok(Some(MemberDeclaration {
            kind: "constructor".to_string(),
            name: "constructor".to_string(),
            identity: format!("constructor:{}", node_text(node, bytes).unwrap_or_default()),
            declaration: Some(dedent_text(
                source[declaration_start..node.end_byte()].trim(),
            )),
            calls: collect_calls(node, bytes),
            ..MemberDeclaration::default()
        })),
        "anonymous_initializer" => Ok(Some(MemberDeclaration {
            kind: "initializer".to_string(),
            name: format!("initializer#{}", node.start_byte()),
            identity: format!("initializer#{}", node.start_byte()),
            declaration: Some(dedent_text(
                source[declaration_start..node.end_byte()].trim(),
            )),
            calls: collect_calls(node, bytes),
            ..MemberDeclaration::default()
        })),
        "enum_entry" => Ok(parse_enum_entry(source, bytes, node, declaration_start)),
        "type_alias" => Ok(parse_type_alias(source, bytes, node, declaration_start)),
        _ => Ok(None),
    }
}

fn parse_function(
    source: &str,
    bytes: &[u8],
    node: Node<'_>,
    declaration_start: usize,
    kind: &str,
) -> Result<MemberDeclaration> {
    let name = function_name(node, bytes).context("Kotlin function declaration missing name")?;
    let body = first_direct_child(node, "function_body");
    let params = first_direct_child(node, "function_value_parameters")
        .and_then(|node| node_text(node, bytes))
        .unwrap_or_else(|| "()".to_string());
    let signature = body
        .map(|body| dedent_text(&header_before(source, declaration_start, body.start_byte())))
        .unwrap_or_else(|| dedent_text(source[declaration_start..node.end_byte()].trim()));
    let (body_text, body_kind, calls) = match body {
        Some(body) => {
            let text = node_text(body, bytes).unwrap_or_default();
            if text.trim_start().starts_with('{') {
                (
                    Some(dedent_text(
                        &source[inner_brace_bounds(source, body).0
                            ..inner_brace_bounds(source, body).1],
                    )),
                    "block".to_string(),
                    collect_calls(body, bytes),
                )
            } else {
                (
                    Some(strip_expression_body(text)),
                    "expression".to_string(),
                    collect_calls(body, bytes),
                )
            }
        }
        None => (None, "none".to_string(), Vec::new()),
    };

    Ok(MemberDeclaration {
        kind: kind.to_string(),
        name: name.clone(),
        identity: format!("{name}{params}"),
        signature: Some(signature),
        body: body_text,
        declaration: Some(body_kind),
        calls,
    })
}

fn parse_property(
    source: &str,
    bytes: &[u8],
    node: Node<'_>,
    declaration_start: usize,
) -> Option<MemberDeclaration> {
    let variable = first_descendant_kind(node, "variable_declaration")?;
    let name = first_descendant_kind(variable, "identifier")
        .or_else(|| first_descendant_kind(variable, "simple_identifier"))
        .and_then(|node| node_text(node, bytes))?;
    Some(MemberDeclaration {
        kind: "property".to_string(),
        name: name.clone(),
        identity: name,
        declaration: Some(dedent_text(
            source[declaration_start..node.end_byte()].trim(),
        )),
        calls: collect_calls(node, bytes),
        ..MemberDeclaration::default()
    })
}

fn parse_enum_entry(
    source: &str,
    bytes: &[u8],
    node: Node<'_>,
    declaration_start: usize,
) -> Option<MemberDeclaration> {
    let name =
        first_descendant_kind(node, "simple_identifier").and_then(|node| node_text(node, bytes))?;
    Some(MemberDeclaration {
        kind: "enum-entry".to_string(),
        name: name.clone(),
        identity: name,
        declaration: Some(dedent_text(
            source[declaration_start..node.end_byte()].trim(),
        )),
        ..MemberDeclaration::default()
    })
}

fn parse_type_alias(
    source: &str,
    bytes: &[u8],
    node: Node<'_>,
    declaration_start: usize,
) -> Option<MemberDeclaration> {
    let name =
        first_descendant_kind(node, "type_identifier").and_then(|node| node_text(node, bytes))?;
    Some(MemberDeclaration {
        kind: "type-alias".to_string(),
        name: name.clone(),
        identity: name,
        declaration: Some(dedent_text(
            source[declaration_start..node.end_byte()].trim(),
        )),
        ..MemberDeclaration::default()
    })
}

fn index_project(parsed: &ParsedProject, indexes: &mut ImportIndexes) {
    for file in &parsed.files {
        let file_id = file_id(&file.path);
        for declaration in &file.declarations {
            index_file_declaration(file, indexes, file_id, None, declaration);
        }
    }
}

fn index_file_declaration(
    file: &ParsedFile,
    indexes: &mut ImportIndexes,
    parent_id: Uuid,
    type_parent_id: Option<Uuid>,
    declaration: &FileDeclaration,
) {
    match declaration {
        FileDeclaration::Type(declaration) => {
            index_type(file, indexes, parent_id, type_parent_id, declaration)
        }
        FileDeclaration::Member(member) => {
            let symbol_id = member_id(parent_id, &member.kind, &member.identity);
            if member.kind == "function" {
                indexes
                    .functions_by_name
                    .entry(member.name.clone())
                    .or_default()
                    .push(symbol_id);
            }
            if !member.calls.is_empty() || member.kind == "function" {
                indexes.parent_by_symbol.insert(symbol_id, parent_id);
                indexes.calls.insert(symbol_id, member.calls.clone());
            }
        }
    }
}

fn index_type(
    file: &ParsedFile,
    indexes: &mut ImportIndexes,
    parent_id: Uuid,
    type_parent_id: Option<Uuid>,
    declaration: &TypeDeclaration,
) {
    let symbol_id = type_id(
        &file.path,
        file.package.as_deref(),
        type_parent_id,
        &declaration.kind,
        &declaration.name,
    );
    indexes
        .types_by_name
        .entry(declaration.name.clone())
        .or_default()
        .push(symbol_id);
    indexes.parent_by_symbol.insert(symbol_id, parent_id);
    for member in &declaration.members {
        match member {
            TypeMember::Type(nested) => {
                index_type(file, indexes, symbol_id, Some(symbol_id), nested)
            }
            TypeMember::Member(member) => {
                let member_symbol_id = member_id(symbol_id, &member.kind, &member.identity);
                if member.kind == "function" {
                    indexes
                        .members_by_key
                        .entry((symbol_id, member.name.clone()))
                        .or_default()
                        .push(member_symbol_id);
                    indexes
                        .members_by_name
                        .entry(member.name.clone())
                        .or_default()
                        .push(member_symbol_id);
                }
                if !member.calls.is_empty() || member.kind == "function" {
                    indexes.parent_by_symbol.insert(member_symbol_id, symbol_id);
                    indexes.calls.insert(member_symbol_id, member.calls.clone());
                }
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
            "handler": "kotlin",
            "path": file.path,
            "preamble": file.preamble,
            "package": file.package,
            "imports": file.imports,
        }),
    }
}

fn declaration_operations(file: &ParsedFile) -> Vec<Operation> {
    let mut operations = Vec::new();
    let file_id = file_id(&file.path);
    for declaration in &file.declarations {
        push_file_declaration(file, file_id, None, declaration, &mut operations);
    }
    operations
}

fn push_file_declaration(
    file: &ParsedFile,
    parent_id: Uuid,
    type_parent_id: Option<Uuid>,
    declaration: &FileDeclaration,
    operations: &mut Vec<Operation>,
) {
    match declaration {
        FileDeclaration::Type(declaration) => {
            push_type_operations(file, parent_id, type_parent_id, declaration, operations)
        }
        FileDeclaration::Member(member) => {
            operations.push(member_operation(file, parent_id, member))
        }
    }
}

fn push_type_operations(
    file: &ParsedFile,
    parent_id: Uuid,
    type_parent_id: Option<Uuid>,
    declaration: &TypeDeclaration,
    operations: &mut Vec<Operation>,
) {
    let symbol_id = type_id(
        &file.path,
        file.package.as_deref(),
        type_parent_id,
        &declaration.kind,
        &declaration.name,
    );
    operations.push(Operation::CreateSymbol {
        symbol_id,
        parent_id: Some(parent_id),
        kind: declaration.kind.clone(),
        name: declaration.name.clone(),
        body: None,
        metadata: json!({
            "signature": declaration.signature,
            "bodyPreamble": declaration.body_preamble,
            "path": file.path,
            "package": file.package,
        }),
    });
    for member in &declaration.members {
        match member {
            TypeMember::Type(nested) => {
                push_type_operations(file, symbol_id, Some(symbol_id), nested, operations)
            }
            TypeMember::Member(member) => {
                operations.push(member_operation(file, symbol_id, member))
            }
        }
    }
}

fn member_operation(file: &ParsedFile, parent_id: Uuid, member: &MemberDeclaration) -> Operation {
    Operation::CreateSymbol {
        symbol_id: member_id(parent_id, &member.kind, &member.identity),
        parent_id: Some(parent_id),
        kind: member.kind.clone(),
        name: member.name.clone(),
        body: member.body.clone(),
        metadata: json!({
            "signature": member.signature.as_deref().unwrap_or(""),
            "declaration": member.declaration.as_deref().unwrap_or(""),
            "identity": member.identity,
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
        CallTarget::Free(name) => sibling_member(indexes, from_symbol_id, name)
            .or_else(|| unique(indexes.functions_by_name.get(name)?))
            .or_else(|| unique(indexes.members_by_name.get(name)?))
            .or_else(|| unique(indexes.types_by_name.get(name)?)),
        CallTarget::This(name) => sibling_member(indexes, from_symbol_id, name),
        CallTarget::Method(name) => unique(indexes.members_by_name.get(name)?),
        CallTarget::Constructor(name) => unique(indexes.types_by_name.get(name)?),
    }
}

fn sibling_member(indexes: &ImportIndexes, from_symbol_id: Uuid, name: &str) -> Option<Uuid> {
    let parent_id = indexes.parent_by_symbol.get(&from_symbol_id)?;
    unique(
        indexes
            .members_by_key
            .get(&(*parent_id, name.to_string()))?,
    )
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
    if node.kind() == "call_expression"
        && let Some(call) = call_target(node, bytes)
    {
        calls.push(call);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_calls_inner(child, bytes, calls);
    }
}

fn call_target(node: Node<'_>, bytes: &[u8]) -> Option<CallTarget> {
    let text = node_text(node, bytes)?;
    let callee = text.split(['(', '{']).next()?.trim();
    if callee.is_empty() {
        return None;
    }
    if let Some((receiver, name)) = callee.rsplit_once('.') {
        let name = simple_identifier(name)?;
        if receiver.trim() == "this" {
            Some(CallTarget::This(name))
        } else {
            Some(CallTarget::Method(name))
        }
    } else {
        let name = simple_identifier(callee)?;
        if name
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_uppercase())
        {
            Some(CallTarget::Constructor(name))
        } else {
            Some(CallTarget::Free(name))
        }
    }
}

fn is_type_kind(kind: &str) -> bool {
    matches!(
        kind,
        "class_declaration" | "object_declaration" | "companion_object"
    )
}

fn is_member_kind(kind: &str) -> bool {
    matches!(
        kind,
        "function_declaration"
            | "property_declaration"
            | "secondary_constructor"
            | "anonymous_initializer"
            | "enum_entry"
            | "type_alias"
    )
}

fn type_kind(node: Node<'_>, source: &str) -> &'static str {
    match node.kind() {
        "object_declaration" | "companion_object" => "object",
        "class_declaration" => {
            let text = source[node.start_byte()..node.end_byte()]
                .chars()
                .take(80)
                .collect::<String>();
            if text.contains("interface ") {
                "interface"
            } else if text.contains("enum class") {
                "enum"
            } else {
                "class"
            }
        }
        _ => "type",
    }
}

fn type_body<'a>(node: Node<'a>) -> Option<Node<'a>> {
    first_direct_child(node, "class_body").or_else(|| first_direct_child(node, "enum_class_body"))
}

fn type_name(node: Node<'_>, bytes: &[u8]) -> Option<String> {
    node.child_by_field_name("name")
        .and_then(|node| node_text(node, bytes))
        .or_else(|| {
            first_direct_child(node, "type_identifier").and_then(|node| node_text(node, bytes))
        })
        .or_else(|| first_direct_child(node, "identifier").and_then(|node| node_text(node, bytes)))
        .or_else(|| (node.kind() == "companion_object").then(|| "Companion".to_string()))
}

fn function_name(node: Node<'_>, bytes: &[u8]) -> Option<String> {
    node.child_by_field_name("name")
        .and_then(|node| node_text(node, bytes))
        .or_else(|| {
            first_direct_child(node, "simple_identifier").and_then(|node| node_text(node, bytes))
        })
        .or_else(|| first_direct_child(node, "identifier").and_then(|node| node_text(node, bytes)))
}

fn unwrap_declaration(mut node: Node<'_>) -> Node<'_> {
    loop {
        if !matches!(
            node.kind(),
            "statement" | "class_member_declaration" | "declaration"
        ) {
            return node;
        }
        let mut cursor = node.walk();
        let named = node
            .children(&mut cursor)
            .filter(|child| child.is_named())
            .collect::<Vec<_>>();
        if named.len() != 1 {
            return node;
        }
        node = named[0];
    }
}

fn first_direct_child<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| child.kind() == kind)
}

fn first_descendant_kind<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    if node.kind() == kind {
        return Some(node);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(found) = first_descendant_kind(child, kind) {
            return Some(found);
        }
    }
    None
}

fn node_text(node: Node<'_>, bytes: &[u8]) -> Option<String> {
    node.utf8_text(bytes).ok().map(ToString::to_string)
}

fn simple_identifier(text: &str) -> Option<String> {
    let text = text.trim().split('<').next().unwrap_or(text).trim();
    let name = text.rsplit('.').next().unwrap_or(text).trim();
    (!name.is_empty()).then(|| {
        name.chars()
            .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
            .collect::<String>()
    })
}

fn header_before(source: &str, start: usize, body_start: usize) -> String {
    strip_open_brace(source[start..body_start].trim_end().to_string())
}

fn strip_open_brace(mut value: String) -> String {
    while value.ends_with('{') {
        value.pop();
        value = value.trim_end().to_string();
    }
    value
}

fn strip_expression_body(value: String) -> String {
    value.trim().trim_start_matches('=').trim().to_string()
}

fn inner_brace_bounds(source: &str, node: Node<'_>) -> (usize, usize) {
    let text = &source[node.start_byte()..node.end_byte()];
    let start = text
        .find('{')
        .map(|offset| node.start_byte() + offset + 1)
        .unwrap_or(node.start_byte());
    let end = text
        .rfind('}')
        .map(|offset| node.start_byte() + offset)
        .unwrap_or(node.end_byte());
    (start.min(end), end)
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
        .filter(|(s, e)| *s >= start && *e <= end)
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
