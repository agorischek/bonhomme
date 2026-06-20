use crate::{
    ids::{file_id, member_id, reference_id, type_id},
    model::{CallTarget, MemberDeclaration, ParsedFile, ParsedProject, TypeDeclaration},
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
    methods_by_key: BTreeMap<(Uuid, String), Uuid>,
    methods_by_name: BTreeMap<String, Vec<Uuid>>,
    parent_by_symbol: BTreeMap<Uuid, Uuid>,
    calls: BTreeMap<Uuid, Vec<CallTarget>>,
}

pub fn import_csharp_files(files: &[RenderedFile]) -> Result<Vec<Operation>> {
    let parsed = parse_csharp_files(files)?;
    operations_from_parsed_project(&parsed)
}

pub(crate) fn parse_csharp_files(files: &[RenderedFile]) -> Result<ParsedProject> {
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
        operations.extend(type_operations(file, &mut indexes));
    }
    operations.extend(reference_operations(&indexes));
    Ok(operations)
}

fn parse_file(path: &str, source: &str) -> Result<ParsedFile> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_c_sharp::LANGUAGE.into())
        .context("failed to load C# tree-sitter grammar")?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow::anyhow!("tree-sitter failed to parse C# source"))?;
    let root = tree.root_node();
    if root.has_error() {
        bail!("{path} is not valid C#");
    }

    let bytes = source.as_bytes();
    let mut file = ParsedFile {
        path: path.to_string(),
        ..ParsedFile::default()
    };
    let mut ranges = Vec::new();
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        match child.kind() {
            "namespace_declaration" => {
                file.namespace = field_text(child, "name", bytes);
                file.namespace_style = Some("block".to_string());
                ranges.push((child.start_byte(), child.end_byte()));
                if let Some(body) = child.child_by_field_name("body") {
                    file.declarations
                        .extend(parse_declaration_list(source, bytes, body)?);
                }
            }
            "file_scoped_namespace_declaration" => {
                file.namespace = field_text(child, "name", bytes);
                file.namespace_style = Some("file".to_string());
                ranges.push((child.start_byte(), child.end_byte()));
            }
            kind if is_type_kind(kind) => {
                file.declarations.push(parse_type(source, bytes, child)?);
                ranges.push((child.start_byte(), child.end_byte()));
            }
            _ => {}
        }
    }
    if file.namespace_style.as_deref() == Some("file") {
        let mut cursor = root.walk();
        for child in root.children(&mut cursor) {
            if is_type_kind(child.kind()) {
                file.declarations.push(parse_type(source, bytes, child)?);
                ranges.push((child.start_byte(), child.end_byte()));
            }
        }
    }
    file.preamble = outside_ranges(source, &ranges);
    Ok(file)
}

fn parse_declaration_list(
    source: &str,
    bytes: &[u8],
    body: Node<'_>,
) -> Result<Vec<TypeDeclaration>> {
    let mut declarations = Vec::new();
    let mut cursor = body.walk();
    for child in body.children(&mut cursor) {
        if is_type_kind(child.kind()) {
            declarations.push(parse_type(source, bytes, child)?);
        }
    }
    Ok(declarations)
}

fn parse_type(source: &str, bytes: &[u8], node: Node<'_>) -> Result<TypeDeclaration> {
    let name = field_text(node, "name", bytes).context("C# type declaration missing name")?;
    let body = node.child_by_field_name("body");
    let mut members = Vec::new();
    let mut ranges = Vec::new();
    if let Some(body) = body {
        let mut cursor = body.walk();
        for child in body.children(&mut cursor) {
            if let Some(member) = parse_member(source, bytes, child)? {
                ranges.push((child.start_byte(), child.end_byte()));
                members.push(member);
            }
        }
    }

    let signature = body
        .map(|body| header_before(source, node.start_byte(), body.start_byte()))
        .unwrap_or_else(|| node_text(node, bytes).unwrap_or_default());
    let body_preamble = body
        .map(|body| {
            let (start, end) = inner_brace_bounds(source, body);
            outside_subranges(source, start, end, &ranges)
        })
        .unwrap_or_default();

    Ok(TypeDeclaration {
        kind: type_kind(node.kind()).to_string(),
        name,
        signature,
        body_preamble,
        members,
    })
}

fn parse_member(source: &str, bytes: &[u8], node: Node<'_>) -> Result<Option<MemberDeclaration>> {
    match node.kind() {
        "method_declaration" => parse_callable(source, bytes, node, "method").map(Some),
        "constructor_declaration" => parse_callable(source, bytes, node, "constructor").map(Some),
        "field_declaration" => Ok(parse_field(source, bytes, node)),
        "property_declaration" => Ok(parse_property(source, bytes, node)),
        _ => Ok(None),
    }
}

fn parse_callable(
    source: &str,
    bytes: &[u8],
    node: Node<'_>,
    kind: &str,
) -> Result<MemberDeclaration> {
    let name = field_text(node, "name", bytes).context("C# callable declaration missing name")?;
    let body = node.child_by_field_name("body");
    let signature = body
        .map(|body| header_before(source, node.start_byte(), body.start_byte()))
        .unwrap_or_else(|| strip_trailing_semicolon(node_text(node, bytes).unwrap_or_default()));
    let (body, body_kind, calls) = match body {
        Some(body) if body.kind() == "block" => (
            Some(dedent_text(
                &source[inner_brace_bounds(source, body).0..inner_brace_bounds(source, body).1],
            )),
            "block".to_string(),
            collect_calls(body, bytes),
        ),
        Some(body) if body.kind() == "arrow_expression_clause" => (
            Some(strip_arrow_clause(
                node_text(body, bytes).unwrap_or_default(),
            )),
            "arrow".to_string(),
            collect_calls(body, bytes),
        ),
        Some(body) => (
            Some(node_text(body, bytes).unwrap_or_default()),
            body.kind().to_string(),
            collect_calls(body, bytes),
        ),
        None => (None, "none".to_string(), Vec::new()),
    };

    Ok(MemberDeclaration {
        kind: kind.to_string(),
        name,
        signature: Some(signature),
        body,
        declaration: Some(body_kind),
        calls,
    })
}

fn parse_field(source: &str, bytes: &[u8], node: Node<'_>) -> Option<MemberDeclaration> {
    let variable = first_descendant_kind(node, "variable_declarator")?;
    let name = field_text(variable, "name", bytes)?;
    Some(MemberDeclaration {
        kind: "field".to_string(),
        name,
        declaration: Some(
            source[node.start_byte()..node.end_byte()]
                .trim()
                .to_string(),
        ),
        ..MemberDeclaration::default()
    })
}

fn parse_property(source: &str, bytes: &[u8], node: Node<'_>) -> Option<MemberDeclaration> {
    let name = field_text(node, "name", bytes)?;
    Some(MemberDeclaration {
        kind: "property".to_string(),
        name,
        declaration: Some(
            source[node.start_byte()..node.end_byte()]
                .trim()
                .to_string(),
        ),
        ..MemberDeclaration::default()
    })
}

fn index_project(parsed: &ParsedProject, indexes: &mut ImportIndexes) {
    for file in &parsed.files {
        for declaration in &file.declarations {
            let type_id = type_id(
                &file.path,
                file.namespace.as_deref(),
                &declaration.kind,
                &declaration.name,
            );
            indexes
                .types_by_name
                .entry(declaration.name.clone())
                .or_default()
                .push(type_id);
            for member in &declaration.members {
                if matches!(member.kind.as_str(), "method" | "constructor") {
                    let member_id = member_id(type_id, &member.kind, &member.name);
                    indexes
                        .methods_by_key
                        .insert((type_id, member.name.clone()), member_id);
                    indexes
                        .methods_by_name
                        .entry(member.name.clone())
                        .or_default()
                        .push(member_id);
                    indexes.parent_by_symbol.insert(member_id, type_id);
                    indexes.calls.insert(member_id, member.calls.clone());
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
            "handler": "csharp",
            "path": file.path,
            "preamble": file.preamble,
            "namespace": file.namespace,
            "namespaceStyle": file.namespace_style,
        }),
    }
}

fn type_operations(file: &ParsedFile, indexes: &mut ImportIndexes) -> Vec<Operation> {
    let mut operations = Vec::new();
    let file_id = file_id(&file.path);
    for declaration in &file.declarations {
        let type_id = type_id(
            &file.path,
            file.namespace.as_deref(),
            &declaration.kind,
            &declaration.name,
        );
        operations.push(Operation::CreateSymbol {
            symbol_id: type_id,
            parent_id: Some(file_id),
            kind: declaration.kind.clone(),
            name: declaration.name.clone(),
            body: None,
            metadata: json!({
                "signature": declaration.signature,
                "bodyPreamble": declaration.body_preamble,
                "path": file.path,
            }),
        });
        for member in &declaration.members {
            operations.push(member_operation(file, type_id, member, indexes));
        }
    }
    operations
}

fn member_operation(
    file: &ParsedFile,
    parent_id: Uuid,
    member: &MemberDeclaration,
    indexes: &mut ImportIndexes,
) -> Operation {
    let symbol_id = member_id(parent_id, &member.kind, &member.name);
    if matches!(member.kind.as_str(), "method" | "constructor") {
        indexes.parent_by_symbol.insert(symbol_id, parent_id);
        indexes.calls.insert(symbol_id, member.calls.clone());
    }
    Operation::CreateSymbol {
        symbol_id,
        parent_id: Some(parent_id),
        kind: member.kind.clone(),
        name: member.name.clone(),
        body: member.body.clone(),
        metadata: json!({
            "signature": member.signature.as_deref().unwrap_or(""),
            "declaration": member.declaration.as_deref().unwrap_or(""),
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
        CallTarget::Free(name) => sibling_method(indexes, from_symbol_id, name)
            .or_else(|| unique(indexes.methods_by_name.get(name)?))
            .or_else(|| unique(indexes.types_by_name.get(name)?)),
        CallTarget::This(name) => sibling_method(indexes, from_symbol_id, name),
        CallTarget::Method(name) => unique(indexes.methods_by_name.get(name)?),
    }
}

fn sibling_method(indexes: &ImportIndexes, from_symbol_id: Uuid, name: &str) -> Option<Uuid> {
    let parent_id = indexes.parent_by_symbol.get(&from_symbol_id)?;
    indexes
        .methods_by_key
        .get(&(*parent_id, name.to_string()))
        .copied()
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
    if node.kind() == "invocation_expression"
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
        "generic_name" => Some(CallTarget::Free(generic_name_identifier(node, bytes)?)),
        "member_access_expression" => {
            let name = field_text(node, "name", bytes)?;
            let expression = node.child_by_field_name("expression")?;
            let receiver = node_text(expression, bytes)?;
            if receiver == "this" || receiver == "base" {
                Some(CallTarget::This(name))
            } else {
                Some(CallTarget::Method(name))
            }
        }
        "qualified_name" => {
            let name = node.named_child(node.named_child_count().saturating_sub(1))?;
            Some(CallTarget::Method(node_text(name, bytes)?))
        }
        _ => None,
    }
}

fn is_type_kind(kind: &str) -> bool {
    matches!(
        kind,
        "class_declaration" | "interface_declaration" | "struct_declaration" | "enum_declaration"
    )
}

fn type_kind(kind: &str) -> &'static str {
    match kind {
        "class_declaration" => "class",
        "interface_declaration" => "interface",
        "struct_declaration" => "struct",
        "enum_declaration" => "enum",
        _ => "type",
    }
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

fn field_text(node: Node<'_>, field: &str, bytes: &[u8]) -> Option<String> {
    node_text(node.child_by_field_name(field)?, bytes)
}

fn node_text(node: Node<'_>, bytes: &[u8]) -> Option<String> {
    node.utf8_text(bytes).ok().map(ToString::to_string)
}

fn generic_name_identifier(node: Node<'_>, bytes: &[u8]) -> Option<String> {
    node.named_child(0)
        .and_then(|child| node_text(child, bytes))
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

fn strip_trailing_semicolon(mut value: String) -> String {
    while value.ends_with(';') {
        value.pop();
        value = value.trim_end().to_string();
    }
    value
}

fn strip_arrow_clause(value: String) -> String {
    value
        .trim()
        .trim_start_matches("=>")
        .trim_end_matches(';')
        .trim()
        .to_string()
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
