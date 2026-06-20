use anyhow::{Result, bail};
use oxc_allocator::Allocator;
use oxc_ast::ast::{FunctionBody, Program};
use oxc_parser::Parser;
use oxc_span::{SourceType, Span};
use std::path::Path;
use uuid::Uuid;

pub(crate) fn with_program<T>(
    path: &str,
    source: &str,
    f: impl for<'a> FnOnce(&'a Program<'a>) -> Result<T>,
) -> Result<T> {
    let allocator = Allocator::new();
    let source_type = SourceType::from_path(Path::new(path)).unwrap_or_else(|_| SourceType::ts());
    let parsed = Parser::new(&allocator, source, source_type).parse();
    if parsed.panicked || !parsed.diagnostics.is_empty() {
        bail!("Oxc failed to parse {path}: {:?}", parsed.diagnostics);
    }
    f(&parsed.program)
}

pub(crate) fn span_text(source: &str, span: Span) -> &str {
    span.source_text(source)
}

pub(crate) fn span_range(span: Span) -> (usize, usize) {
    (span.start as usize, span.end as usize)
}

pub(crate) fn declaration_before_body(
    source: &str,
    start: usize,
    body: &FunctionBody<'_>,
) -> String {
    source[start..body.span.start as usize].trim().to_string()
}

pub(crate) fn class_declaration_before_body(source: &str, start: usize, body: Span) -> String {
    source[start..body.start as usize].trim().to_string()
}

pub(crate) fn body_text(source: &str, body: &FunctionBody<'_>) -> String {
    let start = body.span.start as usize;
    let end = body.span.end as usize;
    if end <= start + 1 {
        return String::new();
    }
    normalize_body(&source[start + 1..end - 1])
}

pub(crate) fn normalize_body(body: &str) -> String {
    body.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn outside_ranges(content: &str, ranges: &[(usize, usize)]) -> String {
    let mut out = String::new();
    let mut cursor = 0;
    for (start, end) in ranges {
        if cursor < *start {
            out.push_str(&content[cursor..*start]);
            out.push('\n');
        }
        cursor = (*end).max(cursor);
    }
    if cursor < content.len() {
        out.push_str(&content[cursor..]);
    }
    out.lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

pub(crate) fn find_file_symbol_id(source: &str) -> Option<Uuid> {
    find_uuid_after(source, "bonhomme:file=")
}

pub(crate) fn find_symbol_id(source: &str) -> Option<Uuid> {
    find_uuid_after(source, "bonhomme:symbol=")
}

pub(crate) fn strip_symbol_comments(source: &str) -> String {
    let mut output = String::new();
    let mut cursor = 0;
    while let Some(relative_start) = source[cursor..].find("/*") {
        let start = cursor + relative_start;
        let Some(relative_end) = source[start + 2..].find("*/") else {
            break;
        };
        let end = start + 2 + relative_end + 2;
        let comment = &source[start..end];
        if comment.contains("bonhomme:symbol=") {
            output.push_str(&source[cursor..start]);
        } else {
            output.push_str(&source[cursor..end]);
        }
        cursor = end;
    }
    output.push_str(&source[cursor..]);
    output.trim().to_string()
}

fn find_uuid_after(source: &str, marker: &str) -> Option<Uuid> {
    let start = source.find(marker)? + marker.len();
    let candidate = source.get(start..start + 36)?;
    Uuid::parse_str(candidate).ok()
}
