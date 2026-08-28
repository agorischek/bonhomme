use crate::{
    ids::{file_id, member_id, nested_type_id, reference_id, type_id},
    model::{
        CallTarget, MemberDeclaration, ParsedFile, ParsedProject, TypeDeclaration, TypeMember,
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
    callables_by_key: BTreeMap<(Uuid, String), Vec<Uuid>>,
    callables_by_name: BTreeMap<String, Vec<Uuid>>,
    constructors_by_type: BTreeMap<String, Vec<Uuid>>,
    parent_by_symbol: BTreeMap<Uuid, Uuid>,
    calls: BTreeMap<Uuid, Vec<CallTarget>>,
}

pub fn import_java_files(files: &[RenderedFile]) -> Result<Vec<Operation>> {
    let parsed = parse_java_files(files)?;
    operations_from_parsed_project(&parsed)
}

pub(crate) fn parse_java_files(files: &[RenderedFile]) -> Result<ParsedProject> {
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
        operations.extend(type_operations(file));
    }
    operations.extend(reference_operations(&indexes));
    Ok(operations)
}

fn parse_file(path: &str, source: &str) -> Result<ParsedFile> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_java::LANGUAGE.into())
        .context("failed to load Java tree-sitter grammar")?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow::anyhow!("tree-sitter failed to parse Java source"))?;
    let root = tree.root_node();
    if root.has_error() {
        bail!("{path} is not valid Java");
    }

    let bytes = source.as_bytes();
    let mut file = ParsedFile {
        path: path.to_string(),
        ..ParsedFile::default()
    };
    let mut ranges = Vec::new();
    let mut leading_cursor = 0;
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        match child.kind() {
            "package_declaration" => {
                file.package = package_name(child, bytes);
                leading_cursor = child.end_byte();
            }
            "import_declaration" => {
                if let Some(import) = node_text(child, bytes) {
                    file.imports.push(import.trim().to_string());
                }
                leading_cursor = child.end_byte();
            }
            kind if is_type_kind(kind) => {
                let declaration_start = leading_cursor.min(child.start_byte());
                let declaration = parse_type(source, bytes, child, declaration_start)?;
                ranges.push((declaration_start, child.end_byte()));
                file.declarations.push(declaration);
                leading_cursor = child.end_byte();
            }
            _ => {}
        }
    }
    file.preamble = outside_ranges(source, &ranges);
    Ok(file)
}

fn parse_type(
    source: &str,
    bytes: &[u8],
    node: Node<'_>,
    declaration_start: usize,
) -> Result<TypeDeclaration> {
    let name = field_text(node, "name", bytes).context("Java type declaration missing name")?;
    let body = node.child_by_field_name("body");
    let mut members = Vec::new();
    let mut ranges = Vec::new();

    if let Some(body) = body {
        let (inner_start, inner_end) = inner_brace_bounds(source, body);
        parse_member_container(
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
        kind: type_kind(node.kind()).to_string(),
        name,
        signature,
        body_preamble,
        members,
    })
}

fn parse_member_container(
    source: &str,
    bytes: &[u8],
    container: Node<'_>,
    start: usize,
    end: usize,
    members: &mut Vec<TypeMember>,
    ranges: &mut Vec<(usize, usize)>,
) -> Result<()> {
    let mut leading_cursor = start;
    let mut initializer_index = 0;
    let mut cursor = container.walk();
    for child in container.children(&mut cursor) {
        if !child.is_named() || child.end_byte() <= start || child.start_byte() >= end {
            continue;
        }
        if child.kind() == "enum_body_declarations" {
            let nested_start =
                skip_leading_enum_body_separator(source, child.start_byte(), child.end_byte());
            if nested_start > child.start_byte() {
                ranges.push((child.start_byte(), nested_start));
            }
            parse_member_container(
                source,
                bytes,
                child,
                nested_start,
                child.end_byte(),
                members,
                ranges,
            )?;
            leading_cursor = child.end_byte();
            continue;
        }

        let declaration_start = leading_cursor.min(child.start_byte());
        if let Some((member, range_end)) = parse_type_member(
            source,
            bytes,
            child,
            declaration_start,
            end,
            &mut initializer_index,
        )? {
            ranges.push((declaration_start, range_end));
            members.push(member);
            leading_cursor = range_end;
        }
    }
    Ok(())
}

fn parse_type_member(
    source: &str,
    bytes: &[u8],
    node: Node<'_>,
    declaration_start: usize,
    body_end: usize,
    initializer_index: &mut usize,
) -> Result<Option<(TypeMember, usize)>> {
    match node.kind() {
        kind if is_type_kind(kind) => Ok(Some((
            TypeMember::Type(parse_type(source, bytes, node, declaration_start)?),
            node.end_byte(),
        ))),
        "method_declaration" => Ok(Some((
            TypeMember::Declaration(parse_callable(
                source,
                bytes,
                node,
                declaration_start,
                "method",
            )?),
            node.end_byte(),
        ))),
        "constructor_declaration" | "compact_constructor_declaration" => Ok(Some((
            TypeMember::Declaration(parse_callable(
                source,
                bytes,
                node,
                declaration_start,
                "constructor",
            )?),
            node.end_byte(),
        ))),
        "field_declaration" | "constant_declaration" => {
            Ok(parse_field(source, bytes, node, declaration_start, "field")
                .map(|member| (TypeMember::Declaration(member), node.end_byte())))
        }
        "annotation_type_element_declaration" => Ok(parse_declaration_member(
            source,
            bytes,
            node,
            declaration_start,
            "annotation-element",
        )
        .map(|member| (TypeMember::Declaration(member), node.end_byte()))),
        "enum_constant" => {
            let range_end = extend_enum_constant_end(source, node.end_byte(), body_end);
            Ok(
                parse_declaration_member(source, bytes, node, declaration_start, "enum-constant")
                    .map(|mut member| {
                        member.declaration =
                            Some(dedent_text(source[declaration_start..range_end].trim()));
                        (TypeMember::Declaration(member), range_end)
                    }),
            )
        }
        "static_initializer" | "block" => {
            *initializer_index += 1;
            let kind = if node.kind() == "static_initializer" {
                "static-initializer"
            } else {
                "initializer"
            };
            Ok(Some((
                TypeMember::Declaration(MemberDeclaration {
                    kind: kind.to_string(),
                    name: format!("{kind}#{initializer_index}"),
                    identity: format!("{kind}#{initializer_index}"),
                    declaration: Some(dedent_text(
                        source[declaration_start..node.end_byte()].trim(),
                    )),
                    calls: collect_calls(node, bytes),
                    ..MemberDeclaration::default()
                }),
                node.end_byte(),
            )))
        }
        _ => Ok(None),
    }
}

fn parse_callable(
    source: &str,
    bytes: &[u8],
    node: Node<'_>,
    declaration_start: usize,
    kind: &str,
) -> Result<MemberDeclaration> {
    let name = field_text(node, "name", bytes).context("Java callable declaration missing name")?;
    let parameters =
        field_text(node, "parameters", bytes).unwrap_or_else(|| "<compact>".to_string());
    let body = node.child_by_field_name("body");
    let signature = body
        .map(|body| dedent_text(&header_before(source, declaration_start, body.start_byte())))
        .unwrap_or_else(|| {
            dedent_text(&strip_trailing_semicolon(
                source[declaration_start..node.end_byte()]
                    .trim()
                    .to_string(),
            ))
        });
    let (body, body_kind, calls) = match body {
        Some(body) if matches!(body.kind(), "block" | "constructor_body") => (
            Some(dedent_text(
                &source[inner_brace_bounds(source, body).0..inner_brace_bounds(source, body).1],
            )),
            body.kind().to_string(),
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
        name: name.clone(),
        identity: format!("{name}{parameters}"),
        signature: Some(signature),
        body,
        declaration: Some(body_kind),
        calls,
    })
}

fn parse_field(
    source: &str,
    bytes: &[u8],
    node: Node<'_>,
    declaration_start: usize,
    kind: &str,
) -> Option<MemberDeclaration> {
    let variable = first_descendant_kind(node, "variable_declarator")?;
    let name = field_text(variable, "name", bytes)?;
    Some(MemberDeclaration {
        kind: kind.to_string(),
        name: name.clone(),
        identity: name,
        declaration: Some(dedent_text(
            source[declaration_start..node.end_byte()].trim(),
        )),
        calls: collect_calls(node, bytes),
        ..MemberDeclaration::default()
    })
}

fn parse_declaration_member(
    source: &str,
    bytes: &[u8],
    node: Node<'_>,
    declaration_start: usize,
    kind: &str,
) -> Option<MemberDeclaration> {
    let name = field_text(node, "name", bytes)?;
    Some(MemberDeclaration {
        kind: kind.to_string(),
        name: name.clone(),
        identity: name,
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
            index_type(file, indexes, file_id, None, declaration);
        }
    }
}

fn index_type(
    file: &ParsedFile,
    indexes: &mut ImportIndexes,
    parent_id: Uuid,
    nested_parent_id: Option<Uuid>,
    declaration: &TypeDeclaration,
) {
    let type_id = declaration_type_id(file, nested_parent_id, declaration);
    indexes
        .types_by_name
        .entry(declaration.name.clone())
        .or_default()
        .push(type_id);

    for member in &declaration.members {
        match member {
            TypeMember::Type(nested) => index_type(file, indexes, type_id, Some(type_id), nested),
            TypeMember::Declaration(member) => {
                let symbol_id = member_id(type_id, &member.kind, &member.identity);
                if matches!(member.kind.as_str(), "method" | "constructor") {
                    indexes
                        .callables_by_key
                        .entry((type_id, member.name.clone()))
                        .or_default()
                        .push(symbol_id);
                    indexes
                        .callables_by_name
                        .entry(member.name.clone())
                        .or_default()
                        .push(symbol_id);
                    if member.kind == "constructor" {
                        indexes
                            .constructors_by_type
                            .entry(declaration.name.clone())
                            .or_default()
                            .push(symbol_id);
                    }
                }
                if !member.calls.is_empty()
                    || matches!(member.kind.as_str(), "method" | "constructor")
                {
                    indexes.parent_by_symbol.insert(symbol_id, type_id);
                    indexes.calls.insert(symbol_id, member.calls.clone());
                }
            }
        }
    }

    indexes.parent_by_symbol.insert(type_id, parent_id);
}

fn file_operation(file: &ParsedFile) -> Operation {
    Operation::CreateSymbol {
        symbol_id: file_id(&file.path),
        parent_id: None,
        kind: "file".to_string(),
        name: file.path.clone(),
        body: None,
        metadata: json!({
            "handler": "java",
            "path": file.path,
            "preamble": file.preamble,
            "package": file.package,
            "imports": file.imports,
        }),
    }
}

fn type_operations(file: &ParsedFile) -> Vec<Operation> {
    let mut operations = Vec::new();
    let file_id = file_id(&file.path);
    for declaration in &file.declarations {
        push_type_operations(file, file_id, None, declaration, &mut operations);
    }
    operations
}

fn push_type_operations(
    file: &ParsedFile,
    parent_id: Uuid,
    nested_parent_id: Option<Uuid>,
    declaration: &TypeDeclaration,
    operations: &mut Vec<Operation>,
) {
    let type_id = declaration_type_id(file, nested_parent_id, declaration);
    let overloaded_callables = overloaded_callable_names(declaration);
    operations.push(Operation::CreateSymbol {
        symbol_id: type_id,
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
                push_type_operations(file, type_id, Some(type_id), nested, operations)
            }
            TypeMember::Declaration(member) => operations.push(member_operation(
                file,
                type_id,
                member,
                overloaded_callables.contains(&member.name),
            )),
        }
    }
}

fn member_operation(
    file: &ParsedFile,
    parent_id: Uuid,
    member: &MemberDeclaration,
    use_identity_name: bool,
) -> Operation {
    let name = if use_identity_name && matches!(member.kind.as_str(), "method" | "constructor") {
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

fn overloaded_callable_names(declaration: &TypeDeclaration) -> BTreeSet<String> {
    let mut counts = BTreeMap::<String, usize>::new();
    for member in &declaration.members {
        let TypeMember::Declaration(member) = member else {
            continue;
        };
        if matches!(member.kind.as_str(), "method" | "constructor") {
            *counts.entry(member.name.clone()).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .filter_map(|(name, count)| (count > 1).then_some(name))
        .collect()
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
        CallTarget::Free(name) => sibling_callable(indexes, from_symbol_id, name)
            .or_else(|| unique(indexes.callables_by_name.get(name)?))
            .or_else(|| unique(indexes.types_by_name.get(name)?)),
        CallTarget::This(name) => sibling_callable(indexes, from_symbol_id, name),
        CallTarget::Method(name) => unique(indexes.callables_by_name.get(name)?),
        CallTarget::Constructor(name) => indexes
            .constructors_by_type
            .get(name)
            .and_then(|ids| unique(ids))
            .or_else(|| unique(indexes.types_by_name.get(name)?)),
    }
}

fn sibling_callable(indexes: &ImportIndexes, from_symbol_id: Uuid, name: &str) -> Option<Uuid> {
    let parent_id = indexes.parent_by_symbol.get(&from_symbol_id)?;
    unique(
        indexes
            .callables_by_key
            .get(&(*parent_id, name.to_string()))?,
    )
}

fn unique(ids: &[Uuid]) -> Option<Uuid> {
    (ids.len() == 1).then_some(ids[0])
}

fn declaration_type_id(
    file: &ParsedFile,
    nested_parent_id: Option<Uuid>,
    declaration: &TypeDeclaration,
) -> Uuid {
    nested_parent_id.map_or_else(
        || {
            type_id(
                &file.path,
                file.package.as_deref(),
                &declaration.kind,
                &declaration.name,
            )
        },
        |parent_id| nested_type_id(parent_id, &declaration.kind, &declaration.name),
    )
}

fn collect_calls(node: Node<'_>, bytes: &[u8]) -> Vec<CallTarget> {
    let mut calls = Vec::new();
    collect_calls_inner(node, bytes, &mut calls);
    calls.sort();
    calls.dedup();
    calls
}

fn collect_calls_inner(node: Node<'_>, bytes: &[u8], calls: &mut Vec<CallTarget>) {
    match node.kind() {
        "method_invocation" => {
            if let Some(name) = field_text(node, "name", bytes) {
                let call = node
                    .child_by_field_name("object")
                    .and_then(|object| node_text(object, bytes))
                    .map_or_else(
                        || CallTarget::Free(name.clone()),
                        |receiver| {
                            if matches!(receiver.as_str(), "this" | "super") {
                                CallTarget::This(name.clone())
                            } else {
                                CallTarget::Method(name.clone())
                            }
                        },
                    );
                calls.push(call);
            }
        }
        "object_creation_expression" => {
            if let Some(class_name) = node
                .child_by_field_name("type")
                .and_then(|node| node_text(node, bytes))
                .and_then(|text| simple_type_name(&text))
            {
                calls.push(CallTarget::Constructor(class_name));
            }
        }
        "method_reference" => {
            if let Some(call) = method_reference_target(node, bytes) {
                calls.push(call);
            }
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_calls_inner(child, bytes, calls);
    }
}

fn method_reference_target(node: Node<'_>, bytes: &[u8]) -> Option<CallTarget> {
    let text = node_text(node, bytes)?;
    let (receiver, method) = text.split_once("::")?;
    if method.trim() == "new" {
        simple_type_name(receiver).map(CallTarget::Constructor)
    } else {
        simple_type_name(method).map(CallTarget::Method)
    }
}

fn is_type_kind(kind: &str) -> bool {
    matches!(
        kind,
        "class_declaration"
            | "record_declaration"
            | "interface_declaration"
            | "annotation_type_declaration"
            | "enum_declaration"
    )
}

fn type_kind(kind: &str) -> &'static str {
    match kind {
        "class_declaration" => "class",
        "record_declaration" => "record",
        "interface_declaration" => "interface",
        "annotation_type_declaration" => "annotation",
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

fn package_name(node: Node<'_>, bytes: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .filter(|child| matches!(child.kind(), "identifier" | "scoped_identifier"))
        .filter_map(|child| node_text(child, bytes))
        .last()
}

fn simple_type_name(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let before_generics = text.split('<').next().unwrap_or(text).trim();
    let without_arrays = before_generics.trim_end_matches("[]").trim();
    let name = without_arrays
        .rsplit(['.', '$'])
        .next()
        .unwrap_or(without_arrays)
        .trim();
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

fn strip_trailing_semicolon(mut value: String) -> String {
    while value.ends_with(';') {
        value.pop();
        value = value.trim_end().to_string();
    }
    value
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

fn extend_enum_constant_end(source: &str, node_end: usize, body_end: usize) -> usize {
    let bytes = source.as_bytes();
    let mut cursor = node_end;
    while cursor < body_end && bytes[cursor].is_ascii_whitespace() {
        cursor += 1;
    }
    if cursor < body_end && matches!(bytes[cursor], b',' | b';') {
        cursor + 1
    } else {
        node_end
    }
}

fn skip_leading_enum_body_separator(source: &str, start: usize, end: usize) -> usize {
    let bytes = source.as_bytes();
    let mut cursor = start;
    let mut last_separator_end = start;
    loop {
        while cursor < end && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor < end && bytes[cursor] == b';' {
            cursor += 1;
            last_separator_end = cursor;
            continue;
        }
        break;
    }
    last_separator_end
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
