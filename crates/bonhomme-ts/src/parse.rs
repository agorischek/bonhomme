use crate::import::calls::{CallTarget, collect_function_calls};
use crate::oxc_parse::{
    DocComments, body_text, class_declaration_before_body, declaration_before_body,
    find_file_symbol_id, find_symbol_id, outside_ranges, span_range, span_text,
    strip_symbol_comments, with_program,
};
use anyhow::Result;
use bonhomme_core::RenderedFile;
use oxc_ast::ast::{
    Class, ClassElement, Declaration, Function, MethodDefinition, MethodDefinitionKind,
    PropertyDefinition, PropertyKey, Statement,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ParsedMethod {
    pub symbol_id: Option<Uuid>,
    pub parent_class_id: Option<Uuid>,
    pub kind: String,
    pub name: String,
    pub signature: String,
    pub body: String,
    pub method_kind: String,
    pub is_static: bool,
    #[serde(default)]
    pub doc: Option<String>,
    #[serde(skip)]
    pub(crate) calls: Vec<CallTarget>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ParsedProperty {
    pub symbol_id: Option<Uuid>,
    pub parent_class_id: Option<Uuid>,
    pub name: String,
    pub declaration: String,
    #[serde(default)]
    pub doc: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ParsedClass {
    pub symbol_id: Option<Uuid>,
    pub name: String,
    pub declaration: String,
    #[serde(default)]
    pub doc: Option<String>,
    pub methods: Vec<ParsedMethod>,
    #[serde(default)]
    pub properties: Vec<ParsedProperty>,
    #[serde(skip)]
    pub(crate) member_order: Vec<ParsedClassMemberIndex>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ParsedFunction {
    pub symbol_id: Option<Uuid>,
    pub name: String,
    pub signature: String,
    pub body: String,
    #[serde(default)]
    pub doc: Option<String>,
    #[serde(skip)]
    pub(crate) calls: Vec<CallTarget>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ParsedFile {
    pub path: String,
    pub file_symbol_id: Option<Uuid>,
    #[serde(default)]
    pub preamble: String,
    pub classes: Vec<ParsedClass>,
    pub functions: Vec<ParsedFunction>,
    #[serde(skip)]
    pub(crate) top_level_order: Vec<ParsedTopLevelIndex>,
}

impl ParsedFile {
    /// Every id recovered from this file's identity comments, across the file symbol, classes,
    /// methods, properties, and top-level functions.
    pub(crate) fn symbol_ids(&self) -> impl Iterator<Item = Uuid> + '_ {
        self.file_symbol_id
            .into_iter()
            .chain(self.classes.iter().filter_map(|class| class.symbol_id))
            .chain(
                self.classes
                    .iter()
                    .flat_map(|class| &class.methods)
                    .filter_map(|method| method.symbol_id),
            )
            .chain(
                self.classes
                    .iter()
                    .flat_map(|class| &class.properties)
                    .filter_map(|property| property.symbol_id),
            )
            .chain(
                self.functions
                    .iter()
                    .filter_map(|function| function.symbol_id),
            )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ParsedTopLevelIndex {
    Class(usize),
    Function(usize),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ParsedClassMemberIndex {
    Method(usize),
    Property(usize),
}

pub(crate) fn parse_files(files: &[RenderedFile]) -> Result<BTreeMap<String, ParsedFile>> {
    let mut by_path = BTreeMap::new();
    for file in files {
        by_path.insert(file.path.clone(), parse_file(file)?);
    }
    Ok(by_path)
}

pub fn parse_file(file: &RenderedFile) -> Result<ParsedFile> {
    let mut classes = Vec::new();
    let mut functions = Vec::new();
    let mut top_level_entries = Vec::new();

    with_program(&file.path, &file.content, |program| {
        let docs = DocComments::from_comments(&file.content, &program.comments);
        for statement in &program.body {
            if let Some(entry) =
                parse_top_level_statement(file, statement, &docs, &mut classes, &mut functions)
            {
                top_level_entries.push(entry);
            }
        }
        Ok(())
    })?;

    top_level_entries.sort_by_key(|entry| entry.start);
    let top_level_ranges = top_level_entries
        .iter()
        .map(|entry| (entry.start, entry.end))
        .collect::<Vec<_>>();
    let top_level_order = top_level_entries
        .into_iter()
        .map(|entry| entry.index)
        .collect();

    Ok(ParsedFile {
        path: file.path.clone(),
        file_symbol_id: find_file_symbol_id(&file.content),
        preamble: normalize_preamble(&outside_ranges(&file.content, &top_level_ranges)),
        classes,
        functions,
        top_level_order,
    })
}

struct ParsedTopLevelEntry {
    start: usize,
    end: usize,
    index: ParsedTopLevelIndex,
}

fn parse_top_level_statement(
    file: &RenderedFile,
    statement: &Statement<'_>,
    docs: &DocComments,
    classes: &mut Vec<ParsedClass>,
    functions: &mut Vec<ParsedFunction>,
) -> Option<ParsedTopLevelEntry> {
    match statement {
        Statement::ClassDeclaration(class) => push_class(
            file,
            class,
            class.span.start as usize,
            class.span,
            docs,
            classes,
        ),
        Statement::FunctionDeclaration(function) => push_function(
            file,
            function,
            function.span.start as usize,
            function.span,
            docs,
            functions,
        ),
        Statement::ExportNamedDeclaration(export) => {
            let Some(declaration) = &export.declaration else {
                return None;
            };
            match declaration {
                Declaration::ClassDeclaration(class) => push_class(
                    file,
                    class,
                    export.span.start as usize,
                    export.span,
                    docs,
                    classes,
                ),
                Declaration::FunctionDeclaration(function) => push_function(
                    file,
                    function,
                    export.span.start as usize,
                    export.span,
                    docs,
                    functions,
                ),
                _ => None,
            }
        }
        Statement::ExportDefaultDeclaration(export) => match &export.declaration {
            oxc_ast::ast::ExportDefaultDeclarationKind::ClassDeclaration(class) => push_class(
                file,
                class,
                export.span.start as usize,
                export.span,
                docs,
                classes,
            ),
            oxc_ast::ast::ExportDefaultDeclarationKind::FunctionDeclaration(function) => {
                push_function(
                    file,
                    function,
                    export.span.start as usize,
                    export.span,
                    docs,
                    functions,
                )
            }
            _ => None,
        },
        _ => None,
    }
}

fn push_class(
    file: &RenderedFile,
    class: &Class<'_>,
    declaration_start: usize,
    range_span: oxc_span::Span,
    docs: &DocComments,
    classes: &mut Vec<ParsedClass>,
) -> Option<ParsedTopLevelEntry> {
    let name = class.id.as_ref().map(|id| id.name.to_string())?;
    let raw_declaration =
        class_declaration_before_body(&file.content, declaration_start, class.body.span);
    let symbol_id = find_symbol_id(&raw_declaration);
    let leading_doc = docs.leading_for(declaration_start, &file.content);
    let (methods, properties, member_order) = parse_class_members(file, class, symbol_id, docs);
    let class_index = classes.len();
    classes.push(ParsedClass {
        symbol_id,
        name,
        declaration: strip_symbol_comments(&raw_declaration),
        doc: leading_doc.map(|(_, doc)| doc.to_string()),
        methods,
        properties,
        member_order,
    });

    let (mut start, end) = span_range(range_span);
    if let Some((doc_start, _)) = leading_doc {
        start = start.min(doc_start);
    }
    Some(ParsedTopLevelEntry {
        start,
        end,
        index: ParsedTopLevelIndex::Class(class_index),
    })
}

fn push_function(
    file: &RenderedFile,
    function: &Function<'_>,
    declaration_start: usize,
    range_span: oxc_span::Span,
    docs: &DocComments,
    functions: &mut Vec<ParsedFunction>,
) -> Option<ParsedTopLevelEntry> {
    let body = function.body.as_ref()?;
    let name = function.id.as_ref().map(|id| id.name.to_string())?;
    let raw_signature = declaration_before_body(&file.content, declaration_start, body);
    let leading_doc = docs.leading_for(declaration_start, &file.content);
    let function_index = functions.len();
    functions.push(ParsedFunction {
        symbol_id: find_symbol_id(&raw_signature),
        name,
        signature: strip_symbol_comments(&raw_signature),
        body: body_text(&file.content, body),
        doc: leading_doc.map(|(_, doc)| doc.to_string()),
        calls: collect_function_calls(function),
    });

    let (mut start, end) = span_range(range_span);
    if let Some((doc_start, _)) = leading_doc {
        start = start.min(doc_start);
    }
    Some(ParsedTopLevelEntry {
        start,
        end,
        index: ParsedTopLevelIndex::Function(function_index),
    })
}

fn parse_class_members(
    file: &RenderedFile,
    class: &Class<'_>,
    parent_class_id: Option<Uuid>,
    docs: &DocComments,
) -> (
    Vec<ParsedMethod>,
    Vec<ParsedProperty>,
    Vec<ParsedClassMemberIndex>,
) {
    let mut methods = Vec::new();
    let mut properties = Vec::new();
    let mut member_order = Vec::new();

    for element in &class.body.body {
        match element {
            ClassElement::MethodDefinition(method) => {
                if let Some(parsed) = parse_method(file, method, parent_class_id, docs) {
                    let index = methods.len();
                    methods.push(parsed);
                    member_order.push(ParsedClassMemberIndex::Method(index));
                }
            }
            ClassElement::PropertyDefinition(property) => {
                if let Some(parsed) = parse_property(file, property, parent_class_id, docs) {
                    let index = properties.len();
                    properties.push(parsed);
                    member_order.push(ParsedClassMemberIndex::Property(index));
                }
            }
            _ => {}
        }
    }

    (methods, properties, member_order)
}

fn parse_method(
    file: &RenderedFile,
    method: &MethodDefinition<'_>,
    parent_class_id: Option<Uuid>,
    docs: &DocComments,
) -> Option<ParsedMethod> {
    let body = method.value.body.as_ref()?;
    let name = match &method.key {
        oxc_ast::ast::PropertyKey::StaticIdentifier(identifier) => identifier.name.to_string(),
        oxc_ast::ast::PropertyKey::StringLiteral(literal) => literal.value.to_string(),
        _ => return None,
    };
    let raw_signature = declaration_before_body(&file.content, method.span.start as usize, body);
    Some(ParsedMethod {
        symbol_id: find_symbol_id(&raw_signature),
        parent_class_id,
        kind: method_symbol_kind(method).to_string(),
        name,
        signature: strip_symbol_comments(&raw_signature),
        body: body_text(&file.content, body),
        method_kind: format!("{:?}", method.kind),
        is_static: method.r#static,
        doc: leading_doc(docs, method.span.start as usize, &file.content),
        calls: collect_function_calls(&method.value),
    })
}

fn parse_property(
    file: &RenderedFile,
    property: &PropertyDefinition<'_>,
    parent_class_id: Option<Uuid>,
    docs: &DocComments,
) -> Option<ParsedProperty> {
    let name = match &property.key {
        PropertyKey::StaticIdentifier(identifier) => identifier.name.to_string(),
        PropertyKey::StringLiteral(literal) => literal.value.to_string(),
        _ => return None,
    };
    // Mirror the importer: read any identity comment from the raw span, then strip comments and
    // normalize the trailing semicolon so an unedited property diffs clean.
    let raw = span_text(&file.content, property.span);
    let raw = raw.trim();
    let symbol_id = find_symbol_id(raw);
    let mut declaration = strip_symbol_comments(raw);
    if !declaration.ends_with(';') {
        declaration.push(';');
    }
    Some(ParsedProperty {
        symbol_id,
        parent_class_id,
        name,
        declaration,
        doc: leading_doc(docs, property.span.start as usize, &file.content),
    })
}

/// The JSDoc block immediately preceding `start`, as owned text, or `None`.
fn leading_doc(docs: &DocComments, start: usize, source: &str) -> Option<String> {
    docs.leading_for(start, source)
        .map(|(_, doc)| doc.to_string())
}

fn method_symbol_kind(method: &MethodDefinition<'_>) -> &'static str {
    match (method.r#static, method.kind) {
        (true, MethodDefinitionKind::Get) => "static-getter",
        (true, MethodDefinitionKind::Set) => "static-setter",
        (true, _) => "static-method",
        (false, MethodDefinitionKind::Get) => "getter",
        (false, MethodDefinitionKind::Set) => "setter",
        (false, _) => "method",
    }
}

fn normalize_preamble(preamble: &str) -> String {
    const GENERATED_BANNER: &str =
        "// Generated by bonhomme. Edit through slices; operation replay is authoritative.";

    preamble
        .lines()
        .filter(|line| {
            let line = line.trim_end();
            line != GENERATED_BANNER && !line.contains("bonhomme:file=")
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}
