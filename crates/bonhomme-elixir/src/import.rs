use crate::{
    ids::{file_id, function_id, module_id, reference_id},
    model::{CallTarget, Declaration, ElixirFunction, ParsedFile, ParsedProject},
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
    modules_by_name: BTreeMap<String, Vec<Uuid>>,
    functions_by_parent_key: BTreeMap<(Uuid, String, usize), Uuid>,
    functions_by_name_arity: BTreeMap<(String, usize), Vec<Uuid>>,
    parent_by_symbol: BTreeMap<Uuid, Uuid>,
    calls: BTreeMap<Uuid, Vec<CallTarget>>,
}

pub fn import_elixir_files(files: &[RenderedFile]) -> Result<Vec<Operation>> {
    let parsed = parse_elixir_files(files)?;
    operations_from_parsed_project(&parsed)
}

pub(crate) fn parse_elixir_files(files: &[RenderedFile]) -> Result<ParsedProject> {
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
        let parent_id = file_id(&file.path);
        operations.extend(declaration_operations(file, parent_id, &file.declarations));
    }
    operations.extend(reference_operations(&indexes));
    Ok(operations)
}

fn parse_file(path: &str, source: &str) -> Result<ParsedFile> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_elixir::LANGUAGE.into())
        .context("failed to load Elixir tree-sitter grammar")?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow::anyhow!("tree-sitter failed to parse Elixir source"))?;
    let root = tree.root_node();
    if root.has_error() {
        bail!("{path} is not valid Elixir");
    }

    let bytes = source.as_bytes();
    let mut declarations = Vec::new();
    let mut functions = Vec::new();
    let mut ranges = Vec::new();
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        if is_module_call(child, bytes) {
            declarations.push(parse_module(source, bytes, child)?);
            ranges.push((child.start_byte(), child.end_byte()));
        } else if is_function_definition(child, bytes)
            && let Some(function) = parse_function_clause(bytes, child)
        {
            push_grouped_function(&mut functions, function);
            ranges.push((child.start_byte(), child.end_byte()));
        }
    }
    declarations.extend(functions.into_iter().map(function_declaration));

    Ok(ParsedFile {
        path: path.to_string(),
        preamble: outside_ranges(source, &ranges),
        declarations,
    })
}

fn parse_module(source: &str, bytes: &[u8], node: Node<'_>) -> Result<Declaration> {
    let name = module_name(node, bytes).context("Elixir module missing name")?;
    let body = find_direct_child(node, "do_block").context("Elixir module missing do block")?;
    let (body_start, body_end) = do_block_inner_bounds(body);

    let mut modules = Vec::new();
    let mut functions = Vec::new();
    let mut ranges = Vec::new();
    let mut cursor = body.walk();
    for child in body.children(&mut cursor) {
        if is_module_call(child, bytes) {
            modules.push(parse_module(source, bytes, child)?);
            ranges.push((child.start_byte(), child.end_byte()));
        } else if is_function_definition(child, bytes)
            && let Some(function) = parse_function_clause(bytes, child)
        {
            push_grouped_function(&mut functions, function);
            ranges.push((child.start_byte(), child.end_byte()));
        }
    }

    Ok(Declaration {
        kind: "module".to_string(),
        name,
        signature: Some(header_before_do(
            source,
            node.start_byte(),
            body.start_byte(),
        )),
        preamble: Some(dedent_text(&outside_subranges(
            source, body_start, body_end, &ranges,
        ))),
        modules,
        functions,
    })
}

fn function_declaration(function: ElixirFunction) -> Declaration {
    Declaration {
        kind: function.kind.clone(),
        name: function.symbol_name(),
        functions: vec![function],
        ..Declaration::default()
    }
}

fn parse_function_clause(bytes: &[u8], node: Node<'_>) -> Option<ElixirFunction> {
    let macro_name = call_identifier(node, bytes)?;
    if !matches!(
        macro_name.as_str(),
        "def" | "defp" | "defmacro" | "defmacrop"
    ) {
        return None;
    }
    let args = find_direct_child(node, "arguments")?;
    let head = function_head_call(args, bytes)?;
    let function_name = call_target_name(head, bytes)?;
    let arity = find_direct_child(head, "arguments")
        .map(|arguments| argument_count(arguments))
        .unwrap_or(0);
    let visibility = if matches!(macro_name.as_str(), "defp" | "defmacrop") {
        "private"
    } else {
        "public"
    };
    let kind = if matches!(macro_name.as_str(), "defmacro" | "defmacrop") {
        "macro"
    } else {
        "function"
    };

    Some(ElixirFunction {
        function_name,
        arity,
        visibility: visibility.to_string(),
        kind: kind.to_string(),
        source: normalize_clause_source(node_text(node, bytes)?.trim()),
        calls: function_body_calls(node, bytes),
    })
}

fn function_head_call<'a>(node: Node<'a>, bytes: &[u8]) -> Option<Node<'a>> {
    match node.kind() {
        "call" if call_identifier(node, bytes).is_some_and(|name| !is_definition_macro(&name)) => {
            Some(node)
        }
        "binary_operator" if operator_text(node, bytes).as_deref() == Some("when") => {
            function_head_call(node.child_by_field_name("left")?, bytes)
        }
        _ => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor).filter(|child| child.is_named()) {
                if let Some(head) = function_head_call(child, bytes) {
                    return Some(head);
                }
            }
            None
        }
    }
}

fn push_grouped_function(functions: &mut Vec<ElixirFunction>, function: ElixirFunction) {
    if let Some(existing) = functions.iter_mut().find(|existing| {
        existing.function_name == function.function_name
            && existing.arity == function.arity
            && existing.visibility == function.visibility
            && existing.kind == function.kind
    }) {
        if !existing.source.trim().is_empty() {
            existing.source.push_str("\n\n");
        }
        existing.source.push_str(function.source.trim());
        existing.calls.extend(function.calls);
        existing.calls.sort();
        existing.calls.dedup();
    } else {
        functions.push(function);
    }
}

fn index_project(parsed: &ParsedProject, indexes: &mut ImportIndexes) {
    for file in &parsed.files {
        let parent_id = file_id(&file.path);
        index_declarations(&file.declarations, parent_id, indexes);
    }
}

fn index_declarations(declarations: &[Declaration], parent_id: Uuid, indexes: &mut ImportIndexes) {
    for declaration in declarations {
        match declaration.kind.as_str() {
            "module" => {
                let symbol_id = module_id(parent_id, &declaration.name);
                indexes
                    .modules_by_name
                    .entry(declaration.name.clone())
                    .or_default()
                    .push(symbol_id);
                index_functions(&declaration.functions, symbol_id, indexes);
                index_declarations(&declaration.modules, symbol_id, indexes);
            }
            "function" | "macro" => {
                index_functions(&declaration.functions, parent_id, indexes);
            }
            _ => {}
        }
    }
}

fn index_functions(functions: &[ElixirFunction], parent_id: Uuid, indexes: &mut ImportIndexes) {
    for function in functions {
        let symbol_id = function_id(
            parent_id,
            &function.function_name,
            function.arity,
            &function.visibility,
        );
        indexes.functions_by_parent_key.insert(
            (parent_id, function.function_name.clone(), function.arity),
            symbol_id,
        );
        indexes
            .functions_by_name_arity
            .entry((function.function_name.clone(), function.arity))
            .or_default()
            .push(symbol_id);
        indexes.parent_by_symbol.insert(symbol_id, parent_id);
        indexes.calls.insert(symbol_id, function.calls.clone());
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
            "handler": "elixir",
            "path": file.path,
            "preamble": file.preamble,
        }),
    }
}

fn declaration_operations(
    file: &ParsedFile,
    parent_id: Uuid,
    declarations: &[Declaration],
) -> Vec<Operation> {
    let mut operations = Vec::new();
    for declaration in declarations {
        match declaration.kind.as_str() {
            "module" => {
                let symbol_id = module_id(parent_id, &declaration.name);
                operations.push(Operation::CreateSymbol {
                    symbol_id,
                    parent_id: Some(parent_id),
                    kind: "module".to_string(),
                    name: declaration.name.clone(),
                    body: None,
                    metadata: json!({
                        "signature": declaration.signature.as_deref().unwrap_or(""),
                        "bodyPreamble": declaration.preamble.as_deref().unwrap_or(""),
                        "path": file.path,
                    }),
                });
                operations.extend(function_operations(file, symbol_id, &declaration.functions));
                operations.extend(declaration_operations(
                    file,
                    symbol_id,
                    &declaration.modules,
                ));
            }
            "function" | "macro" => {
                operations.extend(function_operations(file, parent_id, &declaration.functions));
            }
            _ => {}
        }
    }
    operations
}

fn function_operations(
    file: &ParsedFile,
    parent_id: Uuid,
    functions: &[ElixirFunction],
) -> Vec<Operation> {
    functions
        .iter()
        .map(|function| Operation::CreateSymbol {
            symbol_id: function_id(
                parent_id,
                &function.function_name,
                function.arity,
                &function.visibility,
            ),
            parent_id: Some(parent_id),
            kind: function.kind.clone(),
            name: function.symbol_name(),
            body: Some(function.source.clone()),
            metadata: json!({
                "functionName": function.function_name,
                "arity": function.arity,
                "visibility": function.visibility,
                "path": file.path,
            }),
        })
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
        CallTarget::Local { name, arity } => {
            let parent_id = indexes.parent_by_symbol.get(&from_symbol_id)?;
            indexes
                .functions_by_parent_key
                .get(&(*parent_id, name.clone(), *arity))
                .copied()
                .or_else(|| {
                    unique(
                        indexes
                            .functions_by_name_arity
                            .get(&(name.clone(), *arity))?,
                    )
                })
        }
        CallTarget::Remote {
            module,
            name,
            arity,
        } => {
            let module_id = unique_matching_module(indexes, module)?;
            indexes
                .functions_by_parent_key
                .get(&(module_id, name.clone(), *arity))
                .copied()
        }
    }
}

fn unique_matching_module(indexes: &ImportIndexes, module: &str) -> Option<Uuid> {
    if let Some(id) = indexes
        .modules_by_name
        .get(module)
        .and_then(|ids| unique(ids))
    {
        return Some(id);
    }
    let suffix = format!(".{module}");
    let matches = indexes
        .modules_by_name
        .iter()
        .filter(|(name, _)| name.ends_with(&suffix))
        .flat_map(|(_, ids)| ids.iter().copied())
        .collect::<Vec<_>>();
    unique(&matches)
}

fn unique(ids: &[Uuid]) -> Option<Uuid> {
    if ids.len() == 1 { Some(ids[0]) } else { None }
}

fn function_body_calls(node: Node<'_>, bytes: &[u8]) -> Vec<CallTarget> {
    let mut calls = Vec::new();
    if let Some(do_block) = find_direct_child(node, "do_block") {
        collect_calls_inner(do_block, bytes, 0, &mut calls);
    }
    if let Some(arguments) = find_direct_child(node, "arguments")
        && let Some(keywords) = find_direct_child(arguments, "keywords")
    {
        collect_calls_inner(keywords, bytes, 0, &mut calls);
    }
    calls.sort();
    calls.dedup();
    calls
}

fn collect_calls_inner(
    node: Node<'_>,
    bytes: &[u8],
    piped_extra_arg: usize,
    calls: &mut Vec<CallTarget>,
) {
    if node.kind() == "binary_operator" && operator_text(node, bytes).as_deref() == Some("|>") {
        if let Some(left) = node.child_by_field_name("left") {
            collect_calls_inner(left, bytes, 0, calls);
        }
        if let Some(right) = node.child_by_field_name("right") {
            collect_calls_inner(right, bytes, 1, calls);
        }
        return;
    }

    if node.kind() == "call"
        && let Some(call) = call_target(node, bytes, piped_extra_arg)
    {
        calls.push(call);
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor).filter(|child| child.is_named()) {
        collect_calls_inner(child, bytes, 0, calls);
    }
}

fn call_target(node: Node<'_>, bytes: &[u8], piped_extra_arg: usize) -> Option<CallTarget> {
    let target = node.child_by_field_name("target")?;
    let arity = find_direct_child(node, "arguments")
        .map(|arguments| argument_count(arguments))
        .unwrap_or(0)
        + piped_extra_arg;
    match target.kind() {
        "identifier" | "operator_identifier" => {
            let name = node_text(target, bytes)?;
            if is_special_form(&name) {
                None
            } else {
                Some(CallTarget::Local { name, arity })
            }
        }
        "dot" => {
            let module = node_text(target.child_by_field_name("left")?, bytes)?;
            let name = node_text(target.child_by_field_name("right")?, bytes)?;
            Some(CallTarget::Remote {
                module,
                name,
                arity,
            })
        }
        _ => None,
    }
}

fn is_module_call(node: Node<'_>, bytes: &[u8]) -> bool {
    node.kind() == "call" && call_identifier(node, bytes).as_deref() == Some("defmodule")
}

fn is_function_definition(node: Node<'_>, bytes: &[u8]) -> bool {
    node.kind() == "call"
        && call_identifier(node, bytes).is_some_and(|name| is_definition_macro(&name))
}

fn is_definition_macro(name: &str) -> bool {
    matches!(name, "def" | "defp" | "defmacro" | "defmacrop")
}

fn is_special_form(name: &str) -> bool {
    matches!(
        name,
        "alias"
            | "case"
            | "cond"
            | "def"
            | "defdelegate"
            | "defguard"
            | "defguardp"
            | "defimpl"
            | "defmacro"
            | "defmacrop"
            | "defmodule"
            | "defp"
            | "defprotocol"
            | "for"
            | "fn"
            | "if"
            | "import"
            | "quote"
            | "raise"
            | "receive"
            | "require"
            | "try"
            | "unless"
            | "use"
            | "with"
    )
}

fn module_name(node: Node<'_>, bytes: &[u8]) -> Option<String> {
    let arguments = find_direct_child(node, "arguments")?;
    let mut cursor = arguments.walk();
    arguments
        .children(&mut cursor)
        .find(|child| child.is_named() && child.kind() == "alias")
        .and_then(|child| node_text(child, bytes))
}

fn call_identifier(node: Node<'_>, bytes: &[u8]) -> Option<String> {
    let target = node.child_by_field_name("target")?;
    matches!(target.kind(), "identifier" | "operator_identifier")
        .then(|| node_text(target, bytes))?
}

fn call_target_name(node: Node<'_>, bytes: &[u8]) -> Option<String> {
    let target = node.child_by_field_name("target")?;
    matches!(target.kind(), "identifier" | "operator_identifier")
        .then(|| node_text(target, bytes))?
}

fn argument_count(arguments: Node<'_>) -> usize {
    let mut count = 0;
    let mut cursor = arguments.walk();
    for child in arguments
        .children(&mut cursor)
        .filter(|child| child.is_named())
    {
        if child.kind() == "keywords" {
            continue;
        }
        count += 1;
    }
    count
}

fn find_direct_child<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| child.is_named() && child.kind() == kind)
}

fn operator_text(node: Node<'_>, bytes: &[u8]) -> Option<String> {
    node.child_by_field_name("operator")
        .and_then(|operator| node_text(operator, bytes))
        .or_else(|| {
            let mut cursor = node.walk();
            node.children(&mut cursor)
                .find(|child| !child.is_named())
                .and_then(|operator| node_text(operator, bytes))
        })
}

fn node_text(node: Node<'_>, bytes: &[u8]) -> Option<String> {
    node.utf8_text(bytes).ok().map(ToString::to_string)
}

fn header_before_do(source: &str, start: usize, body_start: usize) -> String {
    source[start..body_start].trim_end().to_string()
}

fn do_block_inner_bounds(node: Node<'_>) -> (usize, usize) {
    let mut start = node.start_byte();
    let mut end = node.end_byte();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "do" {
            start = child.end_byte();
        } else if child.kind() == "end" {
            end = child.start_byte();
        }
    }
    (start, end)
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
            if line.trim().is_empty() {
                String::new()
            } else {
                line.chars().skip(indent).collect::<String>()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn normalize_clause_source(text: &str) -> String {
    let mut lines = text.lines().collect::<Vec<_>>();
    while lines.first().is_some_and(|line| line.trim().is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    if lines.is_empty() {
        return String::new();
    }

    let baseline = lines
        .last()
        .filter(|line| line.trim() == "end")
        .map(|line| leading_indent(line))
        .unwrap_or(0);

    lines
        .into_iter()
        .enumerate()
        .map(|(index, line)| {
            if index == 0 {
                line.trim_start().to_string()
            } else if line.trim().is_empty() {
                String::new()
            } else {
                line.chars().skip(baseline).collect::<String>()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn leading_indent(line: &str) -> usize {
    line.chars()
        .take_while(|ch| *ch == ' ' || *ch == '\t')
        .count()
}

fn outside_ranges(source: &str, ranges: &[(usize, usize)]) -> String {
    outside_subranges(source, 0, source.len(), ranges)
}

fn outside_subranges(source: &str, start: usize, end: usize, ranges: &[(usize, usize)]) -> String {
    let mut sorted = ranges
        .iter()
        .copied()
        .filter(|(range_start, range_end)| *range_end > start && *range_start < end)
        .map(|(range_start, range_end)| (range_start.max(start), range_end.min(end)))
        .collect::<Vec<_>>();
    sorted.sort();

    let mut out = String::new();
    let mut cursor = start;
    for (range_start, range_end) in sorted {
        if range_start > cursor {
            out.push_str(&source[cursor..range_start]);
        }
        cursor = cursor.max(range_end);
    }
    if cursor < end {
        out.push_str(&source[cursor..end]);
    }
    trim_blank_edges(&out)
}

fn trim_blank_edges(text: &str) -> String {
    let mut lines = text.lines().collect::<Vec<_>>();
    while lines.first().is_some_and(|line| line.trim().is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}
