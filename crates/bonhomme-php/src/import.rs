use crate::{
    ids::{file_id, function_id, member_id, reference_id, type_id},
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
    functions_by_name: BTreeMap<String, Vec<Uuid>>,
    methods_by_key: BTreeMap<(Uuid, String), Vec<Uuid>>,
    methods_by_name: BTreeMap<String, Vec<Uuid>>,
    constructors_by_type: BTreeMap<String, Vec<Uuid>>,
    parent_by_symbol: BTreeMap<Uuid, Uuid>,
    calls: BTreeMap<Uuid, Vec<CallTarget>>,
}

pub fn import_php_files(files: &[RenderedFile]) -> Result<Vec<Operation>> {
    let parsed = parse_php_files(files)?;
    operations_from_parsed_project(&parsed)
}

pub(crate) fn parse_php_files(files: &[RenderedFile]) -> Result<ParsedProject> {
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
        .set_language(&tree_sitter_php::LANGUAGE_PHP.into())
        .context("failed to load PHP tree-sitter grammar")?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow::anyhow!("tree-sitter failed to parse PHP source"))?;
    let root = tree.root_node();
    if root.has_error() {
        bail!("{path} is not valid PHP");
    }

    let bytes = source.as_bytes();
    let mut file = ParsedFile {
        path: path.to_string(),
        ..ParsedFile::default()
    };
    let mut ranges = Vec::new();
    let mut leading_cursor = 0;
    let mut block_namespace_start = None;
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        match child.kind() {
            "php_tag" | "text" => {
                leading_cursor = child.end_byte();
            }
            "namespace_definition" => {
                file.namespace = field_text(child, "name", bytes);
                if let Some(body) = child.child_by_field_name("body") {
                    block_namespace_start = Some(child.start_byte());
                    file.namespace_style = Some("block".to_string());
                    file.namespace_header = Some(dedent_text(&header_before(
                        source,
                        child.start_byte(),
                        body.start_byte(),
                    )));
                    let (inner_start, inner_end) = inner_brace_bounds(source, body);
                    parse_file_container(
                        source,
                        bytes,
                        body,
                        inner_start,
                        inner_end,
                        &mut file,
                        &mut ranges,
                    )?;
                    file.namespace_body_preamble =
                        outside_subranges(source, inner_start, inner_end, &ranges);
                    ranges.clear();
                    break;
                } else {
                    leading_cursor = child.end_byte();
                }
            }
            "namespace_use_declaration" => {
                if let Some(import) = node_text(child, bytes) {
                    file.imports.push(import.trim().to_string());
                }
                leading_cursor = child.end_byte();
            }
            kind if is_type_kind(kind) || kind == "function_definition" => {
                let declaration_start = leading_cursor.min(child.start_byte());
                if let Some(declaration) =
                    parse_file_declaration(source, bytes, child, declaration_start)?
                {
                    ranges.push((declaration_start, child.end_byte()));
                    file.declarations.push(declaration);
                    leading_cursor = child.end_byte();
                }
            }
            _ => {}
        }
    }

    file.preamble = if let Some(start) = block_namespace_start {
        dedent_text(source[..start].trim())
    } else {
        outside_ranges(source, &ranges)
    };
    Ok(file)
}

fn parse_file_container(
    source: &str,
    bytes: &[u8],
    container: Node<'_>,
    start: usize,
    end: usize,
    file: &mut ParsedFile,
    ranges: &mut Vec<(usize, usize)>,
) -> Result<()> {
    let mut leading_cursor = start;
    let mut cursor = container.walk();
    for child in container.children(&mut cursor) {
        if !child.is_named() || child.end_byte() <= start || child.start_byte() >= end {
            continue;
        }
        match child.kind() {
            "namespace_use_declaration" => {
                if let Some(import) = node_text(child, bytes) {
                    file.imports.push(import.trim().to_string());
                }
                leading_cursor = child.end_byte();
            }
            kind if is_type_kind(kind) || kind == "function_definition" => {
                let declaration_start = leading_cursor.min(child.start_byte());
                if let Some(declaration) =
                    parse_file_declaration(source, bytes, child, declaration_start)?
                {
                    ranges.push((declaration_start, child.end_byte()));
                    file.declarations.push(declaration);
                    leading_cursor = child.end_byte();
                }
            }
            _ => {}
        }
    }
    Ok(())
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
        "function_definition" => Ok(Some(FileDeclaration::Function(parse_callable(
            source,
            bytes,
            node,
            declaration_start,
            "function",
        )?))),
        _ => Ok(None),
    }
}

fn parse_type(
    source: &str,
    bytes: &[u8],
    node: Node<'_>,
    declaration_start: usize,
) -> Result<TypeDeclaration> {
    let name = field_text(node, "name", bytes).context("PHP type declaration missing name")?;
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
    members: &mut Vec<MemberDeclaration>,
    ranges: &mut Vec<(usize, usize)>,
) -> Result<()> {
    let mut leading_cursor = start;
    let mut cursor = container.walk();
    for child in container.children(&mut cursor) {
        if !child.is_named() || child.end_byte() <= start || child.start_byte() >= end {
            continue;
        }
        let declaration_start = leading_cursor.min(child.start_byte());
        if let Some(member) = parse_member(source, bytes, child, declaration_start)? {
            ranges.push((declaration_start, child.end_byte()));
            members.push(member);
            leading_cursor = child.end_byte();
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
        "method_declaration" => {
            parse_callable(source, bytes, node, declaration_start, "method").map(Some)
        }
        "property_declaration" => Ok(parse_property(source, bytes, node, declaration_start)),
        "const_declaration" | "class_constant_declaration" => {
            Ok(parse_constant(source, bytes, node, declaration_start))
        }
        "enum_case" => Ok(parse_enum_case(source, bytes, node, declaration_start)),
        "use_declaration" => Ok(Some(MemberDeclaration {
            kind: "trait-use".to_string(),
            name: first_named_text(node, bytes).unwrap_or_else(|| "use".to_string()),
            identity: format!("trait-use:{}", node.start_byte()),
            declaration: Some(dedent_text(
                source[declaration_start..node.end_byte()].trim(),
            )),
            ..MemberDeclaration::default()
        })),
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
    let name = field_text(node, "name", bytes).context("PHP callable declaration missing name")?;
    let parameters = field_text(node, "parameters", bytes).unwrap_or_else(|| "()".to_string());
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
        Some(body) if body.kind() == "compound_statement" => (
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

fn parse_property(
    source: &str,
    bytes: &[u8],
    node: Node<'_>,
    declaration_start: usize,
) -> Option<MemberDeclaration> {
    let element = first_descendant_kind(node, "property_element")?;
    let name =
        field_text(element, "name", bytes).map(|name| name.trim_start_matches('$').to_string())?;
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

fn parse_constant(
    source: &str,
    bytes: &[u8],
    node: Node<'_>,
    declaration_start: usize,
) -> Option<MemberDeclaration> {
    let element = first_descendant_kind(node, "const_element")?;
    let name = first_name_text(element, bytes)?;
    Some(MemberDeclaration {
        kind: "constant".to_string(),
        name: name.clone(),
        identity: name,
        declaration: Some(dedent_text(
            source[declaration_start..node.end_byte()].trim(),
        )),
        calls: collect_calls(node, bytes),
        ..MemberDeclaration::default()
    })
}

fn parse_enum_case(
    source: &str,
    bytes: &[u8],
    node: Node<'_>,
    declaration_start: usize,
) -> Option<MemberDeclaration> {
    let name = field_text(node, "name", bytes)?;
    Some(MemberDeclaration {
        kind: "enum-case".to_string(),
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
            match declaration {
                FileDeclaration::Type(declaration) => {
                    index_type(file, indexes, file_id, declaration)
                }
                FileDeclaration::Function(function) => {
                    let symbol_id =
                        function_id(&file.path, file.namespace.as_deref(), &function.identity);
                    indexes
                        .functions_by_name
                        .entry(function.name.clone())
                        .or_default()
                        .push(symbol_id);
                    indexes.parent_by_symbol.insert(symbol_id, file_id);
                    indexes.calls.insert(symbol_id, function.calls.clone());
                }
            }
        }
    }
}

fn index_type(
    file: &ParsedFile,
    indexes: &mut ImportIndexes,
    file_id: Uuid,
    declaration: &TypeDeclaration,
) {
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
    indexes.parent_by_symbol.insert(type_id, file_id);

    for member in &declaration.members {
        let symbol_id = member_id(type_id, &member.kind, &member.identity);
        if member.kind == "method" {
            indexes
                .methods_by_key
                .entry((type_id, member.name.clone()))
                .or_default()
                .push(symbol_id);
            indexes
                .methods_by_name
                .entry(member.name.clone())
                .or_default()
                .push(symbol_id);
            if member.name.eq_ignore_ascii_case("__construct") {
                indexes
                    .constructors_by_type
                    .entry(declaration.name.clone())
                    .or_default()
                    .push(symbol_id);
            }
        }
        if !member.calls.is_empty() || member.kind == "method" {
            indexes.parent_by_symbol.insert(symbol_id, type_id);
            indexes.calls.insert(symbol_id, member.calls.clone());
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
            "handler": "php",
            "path": file.path,
            "preamble": file.preamble,
            "namespace": file.namespace,
            "namespaceStyle": file.namespace_style,
            "namespaceHeader": file.namespace_header,
            "namespaceBodyPreamble": file.namespace_body_preamble,
            "imports": file.imports,
        }),
    }
}

fn declaration_operations(file: &ParsedFile) -> Vec<Operation> {
    let mut operations = Vec::new();
    let file_id = file_id(&file.path);
    let overloaded_functions = overloaded_top_level_functions(file);
    for declaration in &file.declarations {
        match declaration {
            FileDeclaration::Type(declaration) => {
                push_type_operations(file, file_id, declaration, &mut operations)
            }
            FileDeclaration::Function(function) => operations.push(function_operation(
                file,
                file_id,
                function,
                overloaded_functions.contains(&function.name),
            )),
        }
    }
    operations
}

fn push_type_operations(
    file: &ParsedFile,
    file_id: Uuid,
    declaration: &TypeDeclaration,
    operations: &mut Vec<Operation>,
) {
    let type_id = type_id(
        &file.path,
        file.namespace.as_deref(),
        &declaration.kind,
        &declaration.name,
    );
    let overloaded_methods = overloaded_methods(declaration);
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
            "namespace": file.namespace,
        }),
    });

    for member in &declaration.members {
        operations.push(member_operation(
            file,
            type_id,
            member,
            overloaded_methods.contains(&member.name),
        ));
    }
}

fn function_operation(
    file: &ParsedFile,
    parent_id: Uuid,
    function: &MemberDeclaration,
    use_identity_name: bool,
) -> Operation {
    let name = if use_identity_name {
        function.identity.clone()
    } else {
        function.name.clone()
    };
    Operation::CreateSymbol {
        symbol_id: function_id(&file.path, file.namespace.as_deref(), &function.identity),
        parent_id: Some(parent_id),
        kind: "function".to_string(),
        name,
        body: function.body.clone(),
        metadata: json!({
            "signature": function.signature.as_deref().unwrap_or(""),
            "declaration": function.declaration.as_deref().unwrap_or(""),
            "identity": function.identity,
            "originalName": function.name,
            "path": file.path,
        }),
    }
}

fn member_operation(
    file: &ParsedFile,
    parent_id: Uuid,
    member: &MemberDeclaration,
    use_identity_name: bool,
) -> Operation {
    let name = if use_identity_name && member.kind == "method" {
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
        CallTarget::Function(name) => unique(indexes.functions_by_name.get(name)?)
            .or_else(|| unique(indexes.methods_by_name.get(name)?)),
        CallTarget::This(name) => sibling_method(indexes, from_symbol_id, name),
        CallTarget::Method(name) => unique(indexes.methods_by_name.get(name)?),
        CallTarget::Constructor(name) => indexes
            .constructors_by_type
            .get(name)
            .and_then(|ids| unique(ids))
            .or_else(|| unique(indexes.types_by_name.get(name)?)),
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
    match node.kind() {
        "function_call_expression" => {
            if let Some(name) = node
                .child_by_field_name("function")
                .and_then(|node| name_text(node, bytes))
            {
                calls.push(CallTarget::Function(name));
            }
        }
        "member_call_expression" | "nullsafe_member_call_expression" => {
            if let Some(name) = field_text(node, "name", bytes) {
                let call = node
                    .child_by_field_name("object")
                    .and_then(|object| node_text(object, bytes))
                    .map_or_else(
                        || CallTarget::Method(name.clone()),
                        |receiver| {
                            if receiver == "$this" {
                                CallTarget::This(name.clone())
                            } else {
                                CallTarget::Method(name.clone())
                            }
                        },
                    );
                calls.push(call);
            }
        }
        "scoped_call_expression" => {
            if let Some(name) = field_text(node, "name", bytes) {
                let call = node
                    .child_by_field_name("scope")
                    .and_then(|scope| node_text(scope, bytes))
                    .map_or_else(
                        || CallTarget::Method(name.clone()),
                        |scope| {
                            if matches!(scope.as_str(), "self" | "static" | "parent") {
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
            if let Some(class_name) =
                first_named_text(node, bytes).and_then(|text| simple_name(&text))
            {
                calls.push(CallTarget::Constructor(class_name));
            }
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_calls_inner(child, bytes, calls);
    }
}

fn overloaded_top_level_functions(file: &ParsedFile) -> BTreeSet<String> {
    let mut counts = BTreeMap::<String, usize>::new();
    for declaration in &file.declarations {
        let FileDeclaration::Function(function) = declaration else {
            continue;
        };
        *counts.entry(function.name.clone()).or_default() += 1;
    }
    counts
        .into_iter()
        .filter_map(|(name, count)| (count > 1).then_some(name))
        .collect()
}

fn overloaded_methods(declaration: &TypeDeclaration) -> BTreeSet<String> {
    let mut counts = BTreeMap::<String, usize>::new();
    for member in &declaration.members {
        if member.kind == "method" {
            *counts.entry(member.name.clone()).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .filter_map(|(name, count)| (count > 1).then_some(name))
        .collect()
}

fn is_type_kind(kind: &str) -> bool {
    matches!(
        kind,
        "class_declaration" | "interface_declaration" | "trait_declaration" | "enum_declaration"
    )
}

fn type_kind(kind: &str) -> &'static str {
    match kind {
        "class_declaration" => "class",
        "interface_declaration" => "interface",
        "trait_declaration" => "trait",
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
    name_text(node.child_by_field_name(field)?, bytes)
        .or_else(|| node_text(node.child_by_field_name(field)?, bytes))
}

fn node_text(node: Node<'_>, bytes: &[u8]) -> Option<String> {
    node.utf8_text(bytes).ok().map(ToString::to_string)
}

fn name_text(node: Node<'_>, bytes: &[u8]) -> Option<String> {
    node_text(node, bytes).and_then(|text| simple_name(&text))
}

fn first_name_text(node: Node<'_>, bytes: &[u8]) -> Option<String> {
    if node.kind() == "name" {
        return node_text(node, bytes);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(name) = first_name_text(child, bytes) {
            return Some(name);
        }
    }
    None
}

fn first_named_text(node: Node<'_>, bytes: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| {
            matches!(
                child.kind(),
                "name" | "qualified_name" | "relative_name" | "namespace_name"
            )
        })
        .and_then(|child| node_text(child, bytes))
}

fn simple_name(text: &str) -> Option<String> {
    let text = text.trim().trim_start_matches('\\');
    if text.is_empty() {
        return None;
    }
    let name = text.rsplit('\\').next().unwrap_or(text).trim();
    (!name.is_empty()).then(|| name.trim_start_matches('$').to_string())
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
