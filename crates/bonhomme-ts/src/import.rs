pub(crate) mod calls;

use crate::oxc_parse::{
    DocComments, body_text, class_declaration_before_body, declaration_before_body, outside_ranges,
    span_range, span_text, strip_symbol_comments, with_program,
};
use crate::scanner::stable_import_uuid;
use anyhow::Result;
use bonhomme_core::{Operation, RenderedFile};
use oxc_ast::ast::{
    Class, ClassElement, Declaration, Function, MethodDefinition, MethodDefinitionKind,
    PropertyDefinition, PropertyKey, Statement,
};
use serde_json::json;
use uuid::Uuid;

use calls::{CallTarget, CallsBySymbol, ImportIndexes, collect_function_calls, import_references};

#[derive(Clone, Debug)]
struct ParsedTopLevel {
    start: usize,
    end: usize,
    operations: Vec<Operation>,
    calls: CallsBySymbol,
}

pub fn import_typescript_files(files: &[RenderedFile]) -> Result<Vec<Operation>> {
    let mut operations = Vec::new();
    let mut indexes = ImportIndexes::default();

    for file in files {
        let file_id = stable_import_uuid(&format!("file:{}", file.path));
        let mut top_level = Vec::new();
        let mut top_level_ranges = Vec::new();

        with_program(&file.path, &file.content, |program| {
            let docs = DocComments::from_comments(&file.content, &program.comments);
            for statement in &program.body {
                if let Some(entry) =
                    import_top_level_statement(file, file_id, statement, &mut indexes, &docs)?
                {
                    top_level_ranges.push((entry.start, entry.end));
                    top_level.push(entry);
                }
            }
            Ok(())
        })?;

        top_level.sort_by_key(|entry| entry.start);
        top_level_ranges.sort_by_key(|range| range.0);
        operations.push(Operation::CreateSymbol {
            symbol_id: file_id,
            parent_id: None,
            kind: "file".to_string(),
            name: file.path.clone(),
            body: None,
            metadata: json!({
                "handler": "typescript",
                "path": file.path,
                "preamble": outside_ranges(&file.content, &top_level_ranges)
            }),
        });
        for entry in top_level {
            operations.extend(entry.operations);
        }
    }

    operations.extend(import_references(&indexes));
    Ok(operations)
}

fn import_top_level_statement(
    file: &RenderedFile,
    file_id: Uuid,
    statement: &Statement<'_>,
    indexes: &mut ImportIndexes,
    docs: &DocComments,
) -> Result<Option<ParsedTopLevel>> {
    let entry = match statement {
        Statement::ClassDeclaration(class) => {
            import_class(file, file_id, class, class.span.start as usize, class.span, docs)
        }
        Statement::FunctionDeclaration(function) => import_function(
            file,
            file_id,
            function,
            function.span.start as usize,
            function.span,
            docs,
        ),
        Statement::ExportNamedDeclaration(export) => {
            let Some(declaration) = &export.declaration else {
                return Ok(None);
            };
            import_declaration(
                file,
                file_id,
                declaration,
                export.span.start as usize,
                export.span,
                docs,
            )
        }
        Statement::ExportDefaultDeclaration(export) => match &export.declaration {
            oxc_ast::ast::ExportDefaultDeclarationKind::ClassDeclaration(class) => import_class(
                file,
                file_id,
                class,
                export.span.start as usize,
                export.span,
                docs,
            ),
            oxc_ast::ast::ExportDefaultDeclarationKind::FunctionDeclaration(function) => {
                import_function(
                    file,
                    file_id,
                    function,
                    export.span.start as usize,
                    export.span,
                    docs,
                )
            }
            _ => Ok(None),
        },
        _ => Ok(None),
    }?;
    if let Some(entry) = &entry {
        for operation in &entry.operations {
            indexes.index_created_symbol(operation, &entry.calls);
        }
    }
    Ok(entry)
}

fn import_declaration(
    file: &RenderedFile,
    file_id: Uuid,
    declaration: &Declaration<'_>,
    start: usize,
    range_span: oxc_span::Span,
    docs: &DocComments,
) -> Result<Option<ParsedTopLevel>> {
    let entry = match declaration {
        Declaration::ClassDeclaration(class) => {
            import_class(file, file_id, class, start, range_span, docs)?
        }
        Declaration::FunctionDeclaration(function) => {
            import_function(file, file_id, function, start, range_span, docs)?
        }
        _ => None,
    };
    Ok(entry)
}

fn import_class(
    file: &RenderedFile,
    file_id: Uuid,
    class: &Class<'_>,
    declaration_start: usize,
    range_span: oxc_span::Span,
    docs: &DocComments,
) -> Result<Option<ParsedTopLevel>> {
    let Some(name) = class.id.as_ref().map(|id| id.name.to_string()) else {
        return Ok(None);
    };
    let class_id = stable_import_uuid(&format!("class:{}:{name}", file.path));
    let declaration = strip_symbol_comments(&class_declaration_before_body(
        &file.content,
        declaration_start,
        class.body.span,
    ));
    let leading_doc = docs.leading_for(declaration_start, &file.content);
    let mut metadata = json!({
        "exported": declaration.split_whitespace().next() == Some("export"),
        "declaration": declaration
    });
    if let Some((_, doc)) = leading_doc {
        metadata["doc"] = json!(doc);
    }
    let mut operations = vec![Operation::CreateSymbol {
        symbol_id: class_id,
        parent_id: Some(file_id),
        kind: "class".to_string(),
        name: name.clone(),
        body: None,
        metadata,
    }];
    let (children, calls) = import_class_children(file, class_id, class, docs)?;
    operations.extend(children);
    let (mut start, end) = span_range(range_span);
    // Extend the claimed range to cover the leading doc so it is not duplicated into file preamble.
    if let Some((doc_start, _)) = leading_doc {
        start = start.min(doc_start);
    }

    Ok(Some(ParsedTopLevel {
        start,
        end,
        operations,
        calls,
    }))
}

fn import_function(
    file: &RenderedFile,
    file_id: Uuid,
    function: &Function<'_>,
    declaration_start: usize,
    range_span: oxc_span::Span,
    docs: &DocComments,
) -> Result<Option<ParsedTopLevel>> {
    let Some(body) = function.body.as_ref() else {
        return Ok(None);
    };
    let Some(name) = function.id.as_ref().map(|id| id.name.to_string()) else {
        return Ok(None);
    };
    let declaration = strip_symbol_comments(&declaration_before_body(
        &file.content,
        declaration_start,
        body,
    ));
    let function_id = stable_import_uuid(&format!("function:{}:{name}", file.path));
    let leading_doc = docs.leading_for(declaration_start, &file.content);
    let mut metadata = json!({
        "exported": declaration.split_whitespace().next() == Some("export"),
        "declaration": declaration
    });
    if let Some((_, doc)) = leading_doc {
        metadata["doc"] = json!(doc);
    }
    let (mut start, end) = span_range(range_span);
    if let Some((doc_start, _)) = leading_doc {
        start = start.min(doc_start);
    }
    let mut calls = CallsBySymbol::new();
    calls.insert(function_id, collect_function_calls(function));

    Ok(Some(ParsedTopLevel {
        start,
        end,
        operations: vec![Operation::CreateSymbol {
            symbol_id: function_id,
            parent_id: Some(file_id),
            kind: "function".to_string(),
            name,
            body: Some(body_text(&file.content, body)),
            metadata,
        }],
        calls,
    }))
}

fn import_class_children(
    file: &RenderedFile,
    class_id: Uuid,
    class: &Class<'_>,
    docs: &DocComments,
) -> Result<(Vec<Operation>, CallsBySymbol)> {
    let mut children = Vec::new();
    let mut calls = CallsBySymbol::new();
    for element in &class.body.body {
        match element {
            ClassElement::MethodDefinition(method) => {
                if let Some((symbol_id, operation, method_calls)) =
                    import_method(file, class_id, method, docs)
                {
                    calls.insert(symbol_id, method_calls);
                    children.push(operation);
                }
            }
            ClassElement::PropertyDefinition(property) => {
                if let Some(operation) = import_property(file, class_id, property, docs) {
                    children.push(operation);
                }
            }
            _ => {}
        }
    }
    Ok((children, calls))
}

fn import_method(
    file: &RenderedFile,
    class_id: Uuid,
    method: &MethodDefinition<'_>,
    docs: &DocComments,
) -> Option<(Uuid, Operation, Vec<CallTarget>)> {
    let body = method.value.body.as_ref()?;
    let name = property_key_name(&method.key)?;
    let symbol_kind = method_symbol_kind(method);
    let method_id = stable_import_uuid(&format!("{symbol_kind}:{}:{class_id}:{name}", file.path));
    let signature = strip_symbol_comments(&declaration_before_body(
        &file.content,
        method.span.start as usize,
        body,
    ));
    let mut metadata = json!({
        "signature": signature,
        "methodKind": format!("{:?}", method.kind),
        "static": method.r#static,
    });
    if let Some((_, doc)) = docs.leading_for(method.span.start as usize, &file.content) {
        metadata["doc"] = json!(doc);
    }
    Some((
        method_id,
        Operation::CreateSymbol {
            symbol_id: method_id,
            parent_id: Some(class_id),
            kind: symbol_kind.to_string(),
            name,
            body: Some(body_text(&file.content, body)),
            metadata,
        },
        collect_function_calls(&method.value),
    ))
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

fn import_property(
    file: &RenderedFile,
    class_id: Uuid,
    property: &PropertyDefinition<'_>,
    docs: &DocComments,
) -> Option<Operation> {
    let name = property_key_name(&property.key)?;
    let property_id = stable_import_uuid(&format!("property:{}:{class_id}:{name}", file.path));
    let mut declaration = strip_symbol_comments(span_text(&file.content, property.span).trim());
    if !declaration.ends_with(';') {
        declaration.push(';');
    }
    let mut metadata = json!({ "declaration": declaration });
    if let Some((_, doc)) = docs.leading_for(property.span.start as usize, &file.content) {
        metadata["doc"] = json!(doc);
    }
    Some(Operation::CreateSymbol {
        symbol_id: property_id,
        parent_id: Some(class_id),
        kind: "property".to_string(),
        name,
        body: None,
        metadata,
    })
}

fn property_key_name(key: &PropertyKey<'_>) -> Option<String> {
    match key {
        PropertyKey::StaticIdentifier(identifier) => Some(identifier.name.to_string()),
        PropertyKey::StringLiteral(literal) => Some(literal.value.to_string()),
        _ => None,
    }
}
