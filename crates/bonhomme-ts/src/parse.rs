use crate::import::calls::{CallTarget, collect_function_calls};
use crate::oxc_parse::{
    DocComments, body_text, class_declaration_before_body, declaration_before_body,
    find_file_symbol_id, find_symbol_id, span_text, strip_symbol_comments, with_program,
};
use anyhow::Result;
use bonhomme_core::RenderedFile;
use oxc_ast::ast::{
    Class, ClassElement, Declaration, Function, MethodDefinition, PropertyDefinition, PropertyKey,
    Statement,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ParsedMethod {
    pub symbol_id: Option<Uuid>,
    pub parent_class_id: Option<Uuid>,
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
    pub methods: Vec<ParsedMethod>,
    #[serde(default)]
    pub properties: Vec<ParsedProperty>,
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
    pub classes: Vec<ParsedClass>,
    pub functions: Vec<ParsedFunction>,
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

    with_program(&file.path, &file.content, |program| {
        let docs = DocComments::from_comments(&file.content, &program.comments);
        for statement in &program.body {
            parse_top_level_statement(file, statement, &docs, &mut classes, &mut functions);
        }
        Ok(())
    })?;

    Ok(ParsedFile {
        path: file.path.clone(),
        file_symbol_id: find_file_symbol_id(&file.content),
        classes,
        functions,
    })
}

fn parse_top_level_statement(
    file: &RenderedFile,
    statement: &Statement<'_>,
    docs: &DocComments,
    classes: &mut Vec<ParsedClass>,
    functions: &mut Vec<ParsedFunction>,
) {
    match statement {
        Statement::ClassDeclaration(class) => {
            push_class(file, class, class.span.start as usize, docs, classes)
        }
        Statement::FunctionDeclaration(function) => push_function(
            file,
            function,
            function.span.start as usize,
            docs,
            functions,
        ),
        Statement::ExportNamedDeclaration(export) => {
            let Some(declaration) = &export.declaration else {
                return;
            };
            match declaration {
                Declaration::ClassDeclaration(class) => {
                    push_class(file, class, export.span.start as usize, docs, classes);
                }
                Declaration::FunctionDeclaration(function) => {
                    push_function(file, function, export.span.start as usize, docs, functions);
                }
                _ => {}
            }
        }
        Statement::ExportDefaultDeclaration(export) => match &export.declaration {
            oxc_ast::ast::ExportDefaultDeclarationKind::ClassDeclaration(class) => {
                push_class(file, class, export.span.start as usize, docs, classes);
            }
            oxc_ast::ast::ExportDefaultDeclarationKind::FunctionDeclaration(function) => {
                push_function(file, function, export.span.start as usize, docs, functions);
            }
            _ => {}
        },
        _ => {}
    }
}

fn push_class(
    file: &RenderedFile,
    class: &Class<'_>,
    declaration_start: usize,
    docs: &DocComments,
    classes: &mut Vec<ParsedClass>,
) {
    let Some(name) = class.id.as_ref().map(|id| id.name.to_string()) else {
        return;
    };
    let declaration =
        class_declaration_before_body(&file.content, declaration_start, class.body.span);
    let symbol_id = find_symbol_id(&declaration);
    classes.push(ParsedClass {
        symbol_id,
        name,
        methods: parse_methods(file, class, symbol_id, docs),
        properties: parse_properties(file, class, symbol_id, docs),
    });
}

fn push_function(
    file: &RenderedFile,
    function: &Function<'_>,
    declaration_start: usize,
    docs: &DocComments,
    functions: &mut Vec<ParsedFunction>,
) {
    let Some(body) = function.body.as_ref() else {
        return;
    };
    let Some(name) = function.id.as_ref().map(|id| id.name.to_string()) else {
        return;
    };
    let raw_signature = declaration_before_body(&file.content, declaration_start, body);
    functions.push(ParsedFunction {
        symbol_id: find_symbol_id(&raw_signature),
        name,
        signature: strip_symbol_comments(&raw_signature),
        body: body_text(&file.content, body),
        doc: leading_doc(docs, declaration_start, &file.content),
        calls: collect_function_calls(function),
    });
}

fn parse_methods(
    file: &RenderedFile,
    class: &Class<'_>,
    parent_class_id: Option<Uuid>,
    docs: &DocComments,
) -> Vec<ParsedMethod> {
    class
        .body
        .body
        .iter()
        .filter_map(|element| match element {
            ClassElement::MethodDefinition(method) => {
                parse_method(file, method, parent_class_id, docs)
            }
            _ => None,
        })
        .collect()
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
        name,
        signature: strip_symbol_comments(&raw_signature),
        body: body_text(&file.content, body),
        doc: leading_doc(docs, method.span.start as usize, &file.content),
        calls: collect_function_calls(&method.value),
    })
}

fn parse_properties(
    file: &RenderedFile,
    class: &Class<'_>,
    parent_class_id: Option<Uuid>,
    docs: &DocComments,
) -> Vec<ParsedProperty> {
    class
        .body
        .body
        .iter()
        .filter_map(|element| match element {
            ClassElement::PropertyDefinition(property) => {
                parse_property(file, property, parent_class_id, docs)
            }
            _ => None,
        })
        .collect()
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
