use crate::scanner::matching_brace;
use anyhow::{Context, Result};
use bonhomme_core::RenderedFile;
use regex::Regex;
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
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ParsedClass {
    pub symbol_id: Option<Uuid>,
    pub name: String,
    pub methods: Vec<ParsedMethod>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ParsedFunction {
    pub symbol_id: Option<Uuid>,
    pub name: String,
    pub signature: String,
    pub body: String,
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
    /// methods, and top-level functions.
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
    let file_id_re = Regex::new(r"bonhomme:file=([0-9a-fA-F-]{36})")?;
    let class_re = Regex::new(
        r"class\s+([A-Za-z_$][A-Za-z0-9_$]*)\s*(?:/\*\s*bonhomme:symbol=([0-9a-fA-F-]{36})\s*\*/)?\s*\{",
    )?;
    let method_re = Regex::new(concat!(
        r"^\s*((?:public|private|protected|async|static)\s+)*",
        r"(?P<name>[A-Za-z_$][A-Za-z0-9_$]*)\s*",
        r"\((?:[^()]|\([^()]*\))*\)\s*(?::\s*[^/{]+)?\s*",
        r"(?:/\*\s*bonhomme:symbol=(?P<id>[0-9a-fA-F-]{36})\s*\*/)?\s*\{",
    ))?;
    let function_re = Regex::new(concat!(
        r"(?m)^\s*(?P<signature>(?:export\s+)?(?:async\s+)?function\s+",
        r"(?P<name>[A-Za-z_$][A-Za-z0-9_$]*)\s*",
        r"\((?:[^()]|\([^()]*\))*\)\s*(?::\s*[^/{]+)?)",
        r"(?:\s*/\*\s*bonhomme:symbol=(?P<id>[0-9a-fA-F-]{36})\s*\*/)?\s*\{",
    ))?;

    let file_symbol_id = file_id_re
        .captures(&file.content)
        .and_then(|captures| captures.get(1))
        .and_then(|capture| Uuid::parse_str(capture.as_str()).ok());

    let mut classes = Vec::new();
    let bytes = file.content.as_bytes();
    let mut offset = 0;
    let mut class_ranges = Vec::new();

    while let Some(captures) = class_re.captures(&file.content[offset..]) {
        let whole = captures.get(0).expect("class regex has full match");
        let name = captures
            .get(1)
            .expect("class regex captures name")
            .as_str()
            .to_string();
        let symbol_id = captures
            .get(2)
            .and_then(|capture| Uuid::parse_str(capture.as_str()).ok());
        let open_brace = offset + whole.end() - 1;
        let close_brace = matching_brace(bytes, open_brace)
            .with_context(|| format!("class {name} has no matching closing brace"))?;
        let body = &file.content[open_brace + 1..close_brace];
        let methods = parse_methods(body, symbol_id, &method_re)?;
        class_ranges.push((offset + whole.start(), close_brace + 1));

        classes.push(ParsedClass {
            symbol_id,
            name,
            methods,
        });

        offset = close_brace + 1;
    }

    Ok(ParsedFile {
        path: file.path.clone(),
        file_symbol_id,
        classes,
        functions: parse_top_level_functions(&file.content, &class_ranges, &function_re)?,
    })
}

fn parse_top_level_functions(
    content: &str,
    class_ranges: &[(usize, usize)],
    function_re: &Regex,
) -> Result<Vec<ParsedFunction>> {
    let mut functions = Vec::new();
    let bytes = content.as_bytes();

    for captures in function_re.captures_iter(content) {
        let whole = captures.get(0).expect("function regex has full match");
        if class_ranges
            .iter()
            .any(|(start, end)| whole.start() >= *start && whole.start() < *end)
        {
            continue;
        }

        let open_brace = whole.end() - 1;
        let close_brace = matching_brace(bytes, open_brace)
            .with_context(|| format!("function at byte {} has no closing brace", whole.start()))?;
        let name = captures
            .name("name")
            .expect("function regex captures name")
            .as_str()
            .to_string();
        let signature = captures
            .name("signature")
            .expect("function regex captures signature")
            .as_str()
            .trim()
            .to_string();
        let symbol_id = captures
            .name("id")
            .and_then(|capture| Uuid::parse_str(capture.as_str()).ok());
        let body = content[open_brace + 1..close_brace]
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join("\n");

        functions.push(ParsedFunction {
            symbol_id,
            name,
            signature,
            body,
        });
    }

    Ok(functions)
}

fn parse_methods(
    class_body: &str,
    parent_class_id: Option<Uuid>,
    method_re: &Regex,
) -> Result<Vec<ParsedMethod>> {
    let mut methods = Vec::new();
    let mut body_offset = 0;
    let bytes = class_body.as_bytes();

    while let Some(captures) = method_re.captures(&class_body[body_offset..]) {
        let whole = captures.get(0).expect("method regex has full match");
        let absolute_start = body_offset + whole.start();
        let absolute_end = body_offset + whole.end();
        let open_brace = absolute_end - 1;
        let close_brace = matching_brace(bytes, open_brace)
            .with_context(|| format!("method at byte {absolute_start} has no closing brace"))?;
        let signature = class_body[absolute_start..open_brace]
            .replace("/*", "")
            .replace("*/", "")
            .replace(
                captures
                    .name("id")
                    .map(|id| format!(" bonhomme:symbol={}", id.as_str()))
                    .unwrap_or_default()
                    .as_str(),
                "",
            )
            .trim()
            .to_string();
        let name = captures
            .name("name")
            .expect("method regex captures name")
            .as_str()
            .to_string();
        let symbol_id = captures
            .name("id")
            .and_then(|capture| Uuid::parse_str(capture.as_str()).ok());
        let body = class_body[open_brace + 1..close_brace]
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join("\n");

        methods.push(ParsedMethod {
            symbol_id,
            parent_class_id,
            name,
            signature,
            body,
        });

        body_offset = close_brace + 1;
    }

    Ok(methods)
}
