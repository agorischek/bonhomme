use crate::{
    ids::{file_id, member_id, reference_id, type_id},
    model::{
        CallTarget, FileDeclaration, MemberDeclaration, ParsedFile, ParsedProject, TypeDeclaration,
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
    methods_by_key: BTreeMap<(Uuid, String), Vec<Uuid>>,
    methods_by_name: BTreeMap<String, Vec<Uuid>>,
    top_level_by_name: BTreeMap<String, Vec<Uuid>>,
    parent_by_symbol: BTreeMap<Uuid, Uuid>,
    calls: BTreeMap<Uuid, Vec<CallTarget>>,
}

pub fn import_ruby_files(files: &[RenderedFile]) -> Result<Vec<Operation>> {
    let parsed = parse_ruby_files(files)?;
    operations_from_parsed_project(&parsed)
}

pub(crate) fn parse_ruby_files(files: &[RenderedFile]) -> Result<ParsedProject> {
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
        .set_language(&tree_sitter_ruby::LANGUAGE.into())
        .context("failed to load Ruby tree-sitter grammar")?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow::anyhow!("tree-sitter failed to parse Ruby source"))?;
    let root = tree.root_node();
    if root.has_error() {
        bail!("{path} is not valid Ruby");
    }

    let bytes = source.as_bytes();
    let mut declarations = Vec::new();
    let mut ranges = Vec::new();
    let mut leading_cursor = 0;
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        if !child.is_named() {
            continue;
        }
        let declaration_start = leading_cursor.min(child.start_byte());
        if let Some(declaration) = parse_file_declaration(source, bytes, child, declaration_start)?
        {
            ranges.push((declaration_start, child.end_byte()));
            declarations.push(declaration);
            leading_cursor = child.end_byte();
        }
    }

    Ok(ParsedFile {
        path: path.to_string(),
        preamble: outside_ranges(source, &ranges),
        declarations,
    })
}

fn parse_file_declaration(
    source: &str,
    bytes: &[u8],
    node: Node<'_>,
    declaration_start: usize,
) -> Result<Option<FileDeclaration>> {
    match node.kind() {
        "module" | "class" => Ok(Some(FileDeclaration::Type(parse_type(
            source,
            bytes,
            node,
            declaration_start,
        )?))),
        "method" | "singleton_method" => Ok(Some(FileDeclaration::Member(parse_method(
            source,
            bytes,
            node,
            declaration_start,
            "function",
        )?))),
        "assignment" => {
            Ok(parse_assignment(source, bytes, node, declaration_start)
                .map(FileDeclaration::Member))
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
    let name =
        field_text(node, "name", bytes).context("Ruby class/module declaration missing name")?;
    let body = node.child_by_field_name("body");
    let mut declarations = Vec::new();
    let mut members = Vec::new();
    let mut ranges = Vec::new();

    if let Some(body) = body {
        parse_type_body(
            source,
            bytes,
            body,
            &mut declarations,
            &mut members,
            &mut ranges,
        )?;
    }

    let signature = body
        .map(|body| dedent_text(source[declaration_start..body.start_byte()].trim()))
        .unwrap_or_else(|| {
            dedent_text(&strip_trailing_end(
                source[declaration_start..node.end_byte()]
                    .trim()
                    .to_string(),
            ))
        });
    let body_preamble = body
        .map(|body| outside_subranges(source, body.start_byte(), body.end_byte(), &ranges))
        .unwrap_or_default();

    Ok(TypeDeclaration {
        kind: node.kind().to_string(),
        name: simple_name(&name).unwrap_or(name),
        signature,
        body_preamble,
        declarations,
        members,
    })
}

fn parse_type_body(
    source: &str,
    bytes: &[u8],
    body: Node<'_>,
    declarations: &mut Vec<TypeDeclaration>,
    members: &mut Vec<MemberDeclaration>,
    ranges: &mut Vec<(usize, usize)>,
) -> Result<()> {
    let mut leading_cursor = body.start_byte();
    let mut cursor = body.walk();
    for child in body.children(&mut cursor) {
        if !child.is_named() {
            continue;
        }
        let declaration_start = leading_cursor.min(child.start_byte());
        match child.kind() {
            "module" | "class" => {
                declarations.push(parse_type(source, bytes, child, declaration_start)?);
                ranges.push((declaration_start, child.end_byte()));
                leading_cursor = child.end_byte();
            }
            "method" => {
                members.push(parse_method(
                    source,
                    bytes,
                    child,
                    declaration_start,
                    "method",
                )?);
                ranges.push((declaration_start, child.end_byte()));
                leading_cursor = child.end_byte();
            }
            "singleton_method" => {
                members.push(parse_method(
                    source,
                    bytes,
                    child,
                    declaration_start,
                    "singleton-method",
                )?);
                ranges.push((declaration_start, child.end_byte()));
                leading_cursor = child.end_byte();
            }
            "assignment" => {
                if let Some(member) = parse_assignment(source, bytes, child, declaration_start) {
                    members.push(member);
                    ranges.push((declaration_start, child.end_byte()));
                    leading_cursor = child.end_byte();
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn parse_method(
    source: &str,
    bytes: &[u8],
    node: Node<'_>,
    declaration_start: usize,
    kind: &str,
) -> Result<MemberDeclaration> {
    let name = field_text(node, "name", bytes).context("Ruby method declaration missing name")?;
    let parameters = field_text(node, "parameters", bytes).unwrap_or_default();
    let body = node.child_by_field_name("body");
    let signature = body
        .map(|body| dedent_text(source[declaration_start..body.start_byte()].trim()))
        .unwrap_or_else(|| {
            dedent_text(&strip_trailing_end(
                source[declaration_start..node.end_byte()]
                    .trim()
                    .to_string(),
            ))
        });
    let body_text = body.map(|body| dedent_text(&source[body.start_byte()..body.end_byte()]));
    let calls = body
        .map(|body| collect_calls(body, bytes))
        .unwrap_or_default();

    Ok(MemberDeclaration {
        kind: kind.to_string(),
        name: name.clone(),
        identity: format!("{name}{parameters}"),
        signature: Some(signature),
        body: body_text,
        declaration: Some(if body.is_some() { "body" } else { "none" }.to_string()),
        calls,
    })
}

fn parse_assignment(
    source: &str,
    bytes: &[u8],
    node: Node<'_>,
    declaration_start: usize,
) -> Option<MemberDeclaration> {
    let left = node.child_by_field_name("left")?;
    let name = match left.kind() {
        "constant" | "scope_resolution" => field_or_node_text(left, "name", bytes)?,
        "instance_variable" => node_text(left, bytes)?.trim_start_matches('@').to_string(),
        "class_variable" => node_text(left, bytes)?.trim_start_matches("@@").to_string(),
        _ => return None,
    };
    let kind = if matches!(left.kind(), "instance_variable" | "class_variable") {
        "attribute"
    } else {
        "constant"
    };
    Some(MemberDeclaration {
        kind: kind.to_string(),
        name: simple_name(&name).unwrap_or(name.clone()),
        identity: simple_name(&name).unwrap_or(name),
        declaration: Some(dedent_text(
            source[declaration_start..node.end_byte()].trim(),
        )),
        calls: collect_calls(node, bytes),
        ..MemberDeclaration::default()
    })
}

fn index_project(parsed: &ParsedProject, indexes: &mut ImportIndexes) {
    for file in &parsed.files {
        let file_id = file_id(&file.path);
        for declaration in &file.declarations {
            index_file_declaration(indexes, file_id, declaration);
        }
    }
}

fn index_file_declaration(
    indexes: &mut ImportIndexes,
    parent_id: Uuid,
    declaration: &FileDeclaration,
) {
    match declaration {
        FileDeclaration::Type(declaration) => index_type(indexes, parent_id, declaration),
        FileDeclaration::Member(member) => {
            let symbol_id = member_id(parent_id, &member.kind, &member.identity);
            if matches!(
                member.kind.as_str(),
                "function" | "method" | "singleton-method"
            ) {
                indexes
                    .top_level_by_name
                    .entry(member.name.clone())
                    .or_default()
                    .push(symbol_id);
                indexes.parent_by_symbol.insert(symbol_id, parent_id);
                indexes.calls.insert(symbol_id, member.calls.clone());
            }
        }
    }
}

fn index_type(indexes: &mut ImportIndexes, parent_id: Uuid, declaration: &TypeDeclaration) {
    let symbol_id = type_id(parent_id, &declaration.kind, &declaration.name);
    indexes
        .types_by_name
        .entry(declaration.name.clone())
        .or_default()
        .push(symbol_id);
    indexes.parent_by_symbol.insert(symbol_id, parent_id);

    for nested in &declaration.declarations {
        index_type(indexes, symbol_id, nested);
    }
    for member in &declaration.members {
        let member_id = member_id(symbol_id, &member.kind, &member.identity);
        if matches!(member.kind.as_str(), "method" | "singleton-method") {
            indexes
                .methods_by_key
                .entry((symbol_id, member.name.clone()))
                .or_default()
                .push(member_id);
            indexes
                .methods_by_name
                .entry(member.name.clone())
                .or_default()
                .push(member_id);
            indexes.parent_by_symbol.insert(member_id, symbol_id);
            indexes.calls.insert(member_id, member.calls.clone());
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
            "handler": "ruby",
            "path": file.path,
            "preamble": file.preamble,
        }),
    }
}

fn declaration_operations(file: &ParsedFile) -> Vec<Operation> {
    let mut operations = Vec::new();
    let file_id = file_id(&file.path);
    let overloaded_top_level = overloaded_file_members(file);
    for declaration in &file.declarations {
        push_file_declaration_operations(
            file,
            file_id,
            declaration,
            &overloaded_top_level,
            &mut operations,
        );
    }
    operations
}

fn push_file_declaration_operations(
    file: &ParsedFile,
    parent_id: Uuid,
    declaration: &FileDeclaration,
    overloaded_top_level: &BTreeSet<String>,
    operations: &mut Vec<Operation>,
) {
    match declaration {
        FileDeclaration::Type(declaration) => {
            push_type_operations(file, parent_id, declaration, operations)
        }
        FileDeclaration::Member(member) => operations.push(member_operation(
            file,
            parent_id,
            member,
            overloaded_top_level.contains(&member.name),
        )),
    }
}

fn push_type_operations(
    file: &ParsedFile,
    parent_id: Uuid,
    declaration: &TypeDeclaration,
    operations: &mut Vec<Operation>,
) {
    let symbol_id = type_id(parent_id, &declaration.kind, &declaration.name);
    let overloaded_members = overloaded_type_members(declaration);
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
        }),
    });
    for nested in &declaration.declarations {
        push_type_operations(file, symbol_id, nested, operations);
    }
    for member in &declaration.members {
        operations.push(member_operation(
            file,
            symbol_id,
            member,
            overloaded_members.contains(&member.name),
        ));
    }
}

fn member_operation(
    file: &ParsedFile,
    parent_id: Uuid,
    member: &MemberDeclaration,
    use_identity_name: bool,
) -> Operation {
    let name = if use_identity_name
        && matches!(
            member.kind.as_str(),
            "function" | "method" | "singleton-method"
        ) {
        member.identity.clone()
    } else {
        member.name.clone()
    };
    Operation::CreateSymbol {
        symbol_id: member_id(parent_id, &member.kind, &member.identity),
        parent_id: Some(parent_id),
        kind: member.kind.clone(),
        name,
        body: member.body.clone(),
        metadata: json!({
            "signature": member.signature.as_deref().unwrap_or(""),
            "declaration": member.declaration.as_deref().unwrap_or(""),
            "identity": member.identity,
            "originalName": member.name,
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
            .or_else(|| unique(indexes.top_level_by_name.get(name)?))
            .or_else(|| unique(indexes.methods_by_name.get(name)?)),
        CallTarget::This(name) => sibling_method(indexes, from_symbol_id, name),
        CallTarget::Method(name) => unique(indexes.methods_by_name.get(name)?),
        CallTarget::Constructor(name) => unique(indexes.types_by_name.get(name)?),
    }
}

fn sibling_method(indexes: &ImportIndexes, from_symbol_id: Uuid, name: &str) -> Option<Uuid> {
    let parent_id = indexes.parent_by_symbol.get(&from_symbol_id)?;
    unique(
        indexes
            .methods_by_key
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
    if node.kind() == "call"
        && let Some(method) = node
            .child_by_field_name("method")
            .and_then(|node| node_text(node, bytes))
    {
        let call = node
            .child_by_field_name("receiver")
            .and_then(|receiver| node_text(receiver, bytes))
            .map_or_else(
                || CallTarget::Free(method.clone()),
                |receiver| {
                    if method == "new" {
                        simple_name(&receiver)
                            .map(CallTarget::Constructor)
                            .unwrap_or_else(|| CallTarget::Method(method.clone()))
                    } else if receiver == "self" {
                        CallTarget::This(method.clone())
                    } else {
                        CallTarget::Method(method.clone())
                    }
                },
            );
        calls.push(call);
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_calls_inner(child, bytes, calls);
    }
}

fn overloaded_file_members(file: &ParsedFile) -> BTreeSet<String> {
    let mut counts = BTreeMap::<String, usize>::new();
    for declaration in &file.declarations {
        let FileDeclaration::Member(member) = declaration else {
            continue;
        };
        if matches!(
            member.kind.as_str(),
            "function" | "method" | "singleton-method"
        ) {
            *counts.entry(member.name.clone()).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .filter_map(|(name, count)| (count > 1).then_some(name))
        .collect()
}

fn overloaded_type_members(declaration: &TypeDeclaration) -> BTreeSet<String> {
    let mut counts = BTreeMap::<String, usize>::new();
    for member in &declaration.members {
        if matches!(member.kind.as_str(), "method" | "singleton-method") {
            *counts.entry(member.name.clone()).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .filter_map(|(name, count)| (count > 1).then_some(name))
        .collect()
}

fn field_text(node: Node<'_>, field: &str, bytes: &[u8]) -> Option<String> {
    field_or_node_text(node.child_by_field_name(field)?, "name", bytes)
}

fn field_or_node_text(node: Node<'_>, field: &str, bytes: &[u8]) -> Option<String> {
    node.child_by_field_name(field)
        .and_then(|child| node_text(child, bytes))
        .or_else(|| node_text(node, bytes))
}

fn node_text(node: Node<'_>, bytes: &[u8]) -> Option<String> {
    node.utf8_text(bytes).ok().map(ToString::to_string)
}

fn simple_name(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    Some(
        text.rsplit("::")
            .next()
            .unwrap_or(text)
            .trim_start_matches('@')
            .to_string(),
    )
}

fn strip_trailing_end(mut value: String) -> String {
    while value.ends_with("end") {
        value.truncate(value.len() - 3);
        value = value.trim_end().to_string();
    }
    value
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
