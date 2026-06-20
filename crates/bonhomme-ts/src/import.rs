pub(crate) mod calls;

use crate::oxc_parse::{
    body_text, class_declaration_before_body, declaration_before_body, outside_ranges, span_range,
    span_text, strip_symbol_comments, with_program,
};
use crate::scanner::stable_import_uuid;
use anyhow::Result;
use bonhomme_core::{Operation, RenderedFile};
use oxc_ast::ast::{
    Class, ClassElement, Declaration, Function, MethodDefinition, PropertyDefinition, PropertyKey,
    Statement,
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
            for statement in &program.body {
                if let Some(entry) =
                    import_top_level_statement(file, file_id, statement, &mut indexes)?
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
            name: file
                .path
                .rsplit('/')
                .next()
                .unwrap_or(file.path.as_str())
                .to_string(),
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
) -> Result<Option<ParsedTopLevel>> {
    let entry = match statement {
        Statement::ClassDeclaration(class) => {
            import_class(file, file_id, class, class.span.start as usize, class.span)
        }
        Statement::FunctionDeclaration(function) => import_function(
            file,
            file_id,
            function,
            function.span.start as usize,
            function.span,
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
            )
        }
        Statement::ExportDefaultDeclaration(export) => match &export.declaration {
            oxc_ast::ast::ExportDefaultDeclarationKind::ClassDeclaration(class) => import_class(
                file,
                file_id,
                class,
                export.span.start as usize,
                export.span,
            ),
            oxc_ast::ast::ExportDefaultDeclarationKind::FunctionDeclaration(function) => {
                import_function(
                    file,
                    file_id,
                    function,
                    export.span.start as usize,
                    export.span,
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
) -> Result<Option<ParsedTopLevel>> {
    let entry = match declaration {
        Declaration::ClassDeclaration(class) => {
            import_class(file, file_id, class, start, range_span)?
        }
        Declaration::FunctionDeclaration(function) => {
            import_function(file, file_id, function, start, range_span)?
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
    let mut operations = vec![Operation::CreateSymbol {
        symbol_id: class_id,
        parent_id: Some(file_id),
        kind: "class".to_string(),
        name: name.clone(),
        body: None,
        metadata: json!({
            "exported": declaration.split_whitespace().next() == Some("export"),
            "declaration": declaration
        }),
    }];
    let (children, calls) = import_class_children(file, class_id, class)?;
    operations.extend(children);
    let (start, end) = span_range(range_span);

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
    let (start, end) = span_range(range_span);
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
            metadata: json!({
                "exported": declaration.split_whitespace().next() == Some("export"),
                "declaration": declaration
            }),
        }],
        calls,
    }))
}

fn import_class_children(
    file: &RenderedFile,
    class_id: Uuid,
    class: &Class<'_>,
) -> Result<(Vec<Operation>, CallsBySymbol)> {
    let mut children = Vec::new();
    let mut calls = CallsBySymbol::new();
    for element in &class.body.body {
        match element {
            ClassElement::MethodDefinition(method) => {
                if let Some((symbol_id, operation, method_calls)) =
                    import_method(file, class_id, method)
                {
                    calls.insert(symbol_id, method_calls);
                    children.push(operation);
                }
            }
            ClassElement::PropertyDefinition(property) => {
                if let Some(operation) = import_property(file, class_id, property) {
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
) -> Option<(Uuid, Operation, Vec<CallTarget>)> {
    let body = method.value.body.as_ref()?;
    let name = property_key_name(&method.key)?;
    let method_id = stable_import_uuid(&format!("method:{}:{class_id}:{name}", file.path));
    let signature = strip_symbol_comments(&declaration_before_body(
        &file.content,
        method.span.start as usize,
        body,
    ));
    Some((
        method_id,
        Operation::CreateSymbol {
            symbol_id: method_id,
            parent_id: Some(class_id),
            kind: "method".to_string(),
            name,
            body: Some(body_text(&file.content, body)),
            metadata: json!({ "signature": signature }),
        },
        collect_function_calls(&method.value),
    ))
}

fn import_property(
    file: &RenderedFile,
    class_id: Uuid,
    property: &PropertyDefinition<'_>,
) -> Option<Operation> {
    let name = property_key_name(&property.key)?;
    let property_id = stable_import_uuid(&format!("property:{}:{class_id}:{name}", file.path));
    let mut declaration = strip_symbol_comments(span_text(&file.content, property.span).trim());
    if !declaration.ends_with(';') {
        declaration.push(';');
    }
    Some(Operation::CreateSymbol {
        symbol_id: property_id,
        parent_id: Some(class_id),
        kind: "property".to_string(),
        name,
        body: None,
        metadata: json!({"declaration": declaration}),
    })
}

fn property_key_name(key: &PropertyKey<'_>) -> Option<String> {
    match key {
        PropertyKey::StaticIdentifier(identifier) => Some(identifier.name.to_string()),
        PropertyKey::StringLiteral(literal) => Some(literal.value.to_string()),
        _ => None,
    }
}
