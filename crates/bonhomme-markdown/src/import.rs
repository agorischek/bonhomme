use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use bonhomme_core::{Operation, RenderedFile};
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Parser, Tag, TagEnd};
use serde_json::json;
use uuid::Uuid;

use crate::{
    ids::{code_block_id, file_id, frontmatter_id, image_id, link_id, reference_id, section_id},
    model::{
        CODE_BLOCK_KIND, EMBEDS_KIND, FRONTMATTER_KIND, IMAGE_KIND, LINK_KIND, LINKS_TO_KIND,
        MODEL_VERSION, SECTION_KIND,
    },
};

pub fn import_markdown_files(files: &[RenderedFile]) -> Result<Vec<Operation>> {
    let mut documents = files.iter().map(parse_document).collect::<Vec<_>>();
    documents.sort_by(|left, right| left.path.cmp(&right.path));

    let mut operations = Vec::new();
    for document in &documents {
        operations.extend(document_operations(document));
    }
    operations.extend(reference_operations(&documents));
    Ok(operations)
}

#[derive(Clone, Debug)]
struct ParsedDocument {
    path: String,
    file_id: Uuid,
    frontmatter: Option<Frontmatter>,
    preamble: String,
    sections: Vec<Section>,
    code_blocks: Vec<CodeBlock>,
    links: Vec<Link>,
}

#[derive(Clone, Debug)]
struct Frontmatter {
    body: String,
    format: String,
    end: usize,
}

#[derive(Clone, Debug)]
struct Section {
    id: Uuid,
    parent_id: Uuid,
    name: String,
    title: String,
    identity_path: String,
    anchor: String,
    level: usize,
    start: usize,
    end: usize,
    body: String,
}

#[derive(Clone, Debug)]
struct CodeBlock {
    id: Uuid,
    parent_id: Uuid,
    name: String,
    body: String,
    info: String,
    language: String,
}

#[derive(Clone, Debug)]
struct Link {
    id: Uuid,
    parent_id: Uuid,
    kind: String,
    name: String,
    body: String,
    destination: String,
    title: String,
    text: String,
}

#[derive(Clone, Debug)]
struct HeadingAnchor {
    level: usize,
    start: usize,
    title: String,
    explicit_anchor: Option<String>,
}

#[derive(Clone, Debug)]
struct SectionSeed {
    id: Uuid,
    parent_index: Option<usize>,
    name: String,
    title: String,
    identity_path: String,
    anchor: String,
    level: usize,
    start: usize,
}

#[derive(Clone, Debug)]
struct RawCodeBlock {
    start: usize,
    end: usize,
    info: String,
    language: String,
}

#[derive(Clone, Debug)]
struct RawLink {
    image: bool,
    start: usize,
    end: usize,
    destination: String,
    title: String,
    text: String,
}

#[derive(Clone, Debug)]
struct LinkCapture {
    image: bool,
    start: usize,
    destination: String,
    title: String,
    text: String,
}

fn parse_document(file: &RenderedFile) -> ParsedDocument {
    let path = file.path.clone();
    let file_id = file_id(&path);
    let frontmatter = parse_frontmatter(&file.content);
    let content_start = frontmatter.as_ref().map(|item| item.end).unwrap_or(0);
    let sections = parse_sections(&path, file_id, &file.content, content_start);
    let first_section_start = sections
        .first()
        .map(|section| section.start)
        .unwrap_or(file.content.len());
    let preamble = file.content[content_start..first_section_start].to_string();
    let (code_blocks, links) = parse_semantic_children(&path, file_id, &file.content, &sections);

    ParsedDocument {
        path,
        file_id,
        frontmatter,
        preamble,
        sections,
        code_blocks,
        links,
    }
}

fn document_operations(document: &ParsedDocument) -> Vec<Operation> {
    let mut operations = vec![Operation::CreateSymbol {
        symbol_id: document.file_id,
        parent_id: None,
        kind: "file".to_string(),
        name: document.path.clone(),
        body: None,
        metadata: json!({
            "handler": "markdown",
            "path": document.path,
            "preamble": document.preamble,
            "model": MODEL_VERSION,
        }),
    }];

    if let Some(frontmatter) = &document.frontmatter {
        operations.push(Operation::CreateSymbol {
            symbol_id: frontmatter_id(&document.path),
            parent_id: Some(document.file_id),
            kind: FRONTMATTER_KIND.to_string(),
            name: "frontmatter".to_string(),
            body: Some(frontmatter.body.clone()),
            metadata: json!({ "format": frontmatter.format }),
        });
    }

    for section in &document.sections {
        operations.push(Operation::CreateSymbol {
            symbol_id: section.id,
            parent_id: Some(section.parent_id),
            kind: SECTION_KIND.to_string(),
            name: section.name.clone(),
            body: Some(section.body.clone()),
            metadata: json!({
                "level": section.level,
                "title": section.title,
                "anchor": section.anchor,
                "identityPath": section.identity_path,
            }),
        });
    }

    for block in &document.code_blocks {
        operations.push(Operation::CreateSymbol {
            symbol_id: block.id,
            parent_id: Some(block.parent_id),
            kind: CODE_BLOCK_KIND.to_string(),
            name: block.name.clone(),
            body: Some(block.body.clone()),
            metadata: json!({
                "info": block.info,
                "language": block.language,
            }),
        });
    }

    for link in &document.links {
        operations.push(Operation::CreateSymbol {
            symbol_id: link.id,
            parent_id: Some(link.parent_id),
            kind: link.kind.clone(),
            name: link.name.clone(),
            body: Some(link.body.clone()),
            metadata: json!({
                "destination": link.destination,
                "title": link.title,
                "text": link.text,
            }),
        });
    }

    operations
}

fn reference_operations(documents: &[ParsedDocument]) -> Vec<Operation> {
    let file_ids = documents
        .iter()
        .map(|document| (document.path.clone(), document.file_id))
        .collect::<BTreeMap<_, _>>();
    let section_ids = documents
        .iter()
        .flat_map(|document| {
            document
                .sections
                .iter()
                .map(|section| ((document.path.clone(), section.anchor.clone()), section.id))
        })
        .collect::<BTreeMap<_, _>>();

    let mut seen = BTreeSet::new();
    let mut operations = Vec::new();
    for document in documents {
        for link in &document.links {
            let Some(target_id) = resolve_destination(document, link, &file_ids, &section_ids)
            else {
                continue;
            };
            let kind = if link.kind == IMAGE_KIND {
                EMBEDS_KIND
            } else {
                LINKS_TO_KIND
            };
            if target_id == link.id || !seen.insert((link.id, target_id, kind.to_string())) {
                continue;
            }
            operations.push(Operation::CreateReference {
                reference_id: reference_id(link.id, target_id, kind),
                from_symbol_id: link.id,
                to_symbol_id: target_id,
                kind: kind.to_string(),
            });
        }
    }
    operations
}

fn parse_frontmatter(content: &str) -> Option<Frontmatter> {
    let first_line_end = content.find('\n').map(|index| index + 1)?;
    let marker = trim_line_ending(&content[..first_line_end]);
    let format = match marker {
        "---" => "yaml",
        "+++" => "toml",
        _ => return None,
    };

    let mut offset = first_line_end;
    for line in content[first_line_end..].split_inclusive('\n') {
        let end = offset + line.len();
        if trim_line_ending(line) == marker {
            return Some(Frontmatter {
                body: content[..end].to_string(),
                format: format.to_string(),
                end,
            });
        }
        offset = end;
    }

    let trailing = &content[offset..];
    if !trailing.is_empty() && trim_line_ending(trailing) == marker {
        return Some(Frontmatter {
            body: content.to_string(),
            format: format.to_string(),
            end: content.len(),
        });
    }

    None
}

fn parse_sections(path: &str, file_id: Uuid, content: &str, content_start: usize) -> Vec<Section> {
    let anchors = heading_anchors(content)
        .into_iter()
        .filter(|anchor| anchor.start >= content_start)
        .collect::<Vec<_>>();
    if anchors.is_empty() {
        return Vec::new();
    }

    let mut seeds = Vec::new();
    let mut stack: Vec<(usize, String, usize)> = Vec::new();
    let mut identity_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut sibling_counts: BTreeMap<(Option<String>, String), usize> = BTreeMap::new();
    let mut anchor_counts: BTreeMap<String, usize> = BTreeMap::new();

    for anchor in &anchors {
        while let Some((level, _, _)) = stack.last() {
            if *level >= anchor.level {
                stack.pop();
            } else {
                break;
            }
        }

        let parent_index = stack.last().map(|(_, _, index)| *index);
        let parent_identity = stack.last().map(|(_, identity, _)| identity.clone());
        let mut path_titles = stack
            .iter()
            .map(|(_, identity, _)| identity.clone())
            .collect::<Vec<_>>();
        path_titles.push(anchor.title.clone());

        let base_identity = path_titles.join(" > ");
        let identity_count = identity_counts.entry(base_identity.clone()).or_insert(0);
        *identity_count += 1;
        let identity_path = if *identity_count == 1 {
            base_identity
        } else {
            format!("{base_identity} ({identity_count})")
        };

        let sibling_key = (parent_identity.clone(), anchor.title.clone());
        let sibling_count = sibling_counts.entry(sibling_key).or_insert(0);
        *sibling_count += 1;
        let name = if *sibling_count == 1 {
            anchor.title.clone()
        } else {
            format!("{} ({sibling_count})", anchor.title)
        };

        let base_anchor = anchor
            .explicit_anchor
            .as_deref()
            .map(normalize_anchor)
            .filter(|anchor| !anchor.is_empty())
            .unwrap_or_else(|| slug(&anchor.title));
        let anchor_count = anchor_counts.entry(base_anchor.clone()).or_insert(0);
        let unique_anchor = if *anchor_count == 0 {
            base_anchor
        } else {
            format!("{base_anchor}-{anchor_count}")
        };
        *anchor_count += 1;

        let index = seeds.len();
        seeds.push(SectionSeed {
            id: section_id(path, &identity_path),
            parent_index,
            name,
            title: anchor.title.clone(),
            identity_path: identity_path.clone(),
            anchor: unique_anchor,
            level: anchor.level,
            start: anchor.start,
        });
        stack.push((anchor.level, identity_path, index));
    }

    let mut sections = Vec::new();
    for (index, seed) in seeds.iter().enumerate() {
        let end = seeds
            .iter()
            .skip(index + 1)
            .find(|candidate| candidate.level <= seed.level)
            .map(|candidate| candidate.start)
            .unwrap_or(content.len());
        let body_end = seeds
            .iter()
            .filter(|candidate| candidate.parent_index == Some(index))
            .map(|candidate| candidate.start)
            .min()
            .unwrap_or(end);
        let parent_id = seed
            .parent_index
            .map(|parent| seeds[parent].id)
            .unwrap_or(file_id);

        sections.push(Section {
            id: seed.id,
            parent_id,
            name: seed.name.clone(),
            title: seed.title.clone(),
            identity_path: seed.identity_path.clone(),
            anchor: seed.anchor.clone(),
            level: seed.level,
            start: seed.start,
            end,
            body: content[seed.start..body_end].to_string(),
        });
    }

    sections
}

fn heading_anchors(content: &str) -> Vec<HeadingAnchor> {
    let mut anchors = Vec::new();
    let mut current: Option<(usize, usize, String, Option<String>)> = None;

    for (event, range) in Parser::new(content).into_offset_iter() {
        match event {
            Event::Start(Tag::Heading { level, id, .. }) => {
                current = Some((
                    heading_level(level),
                    range.start,
                    String::new(),
                    id.as_ref().map(|id| id.to_string()),
                ));
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some((level, start, title, explicit_anchor)) = current.take() {
                    anchors.push(HeadingAnchor {
                        level,
                        start,
                        title: title.trim().to_string(),
                        explicit_anchor,
                    });
                }
            }
            Event::Text(text) | Event::Code(text) => {
                if let Some((_, _, title, _)) = &mut current {
                    title.push_str(&text);
                }
            }
            _ => {}
        }
    }

    anchors
}

fn parse_semantic_children(
    path: &str,
    file_id: Uuid,
    content: &str,
    sections: &[Section],
) -> (Vec<CodeBlock>, Vec<Link>) {
    let (raw_blocks, raw_links) = raw_semantic_children(content);
    let mut name_counts: BTreeMap<(Uuid, String, String), usize> = BTreeMap::new();
    let mut occurrence_counts: BTreeMap<(String, String), usize> = BTreeMap::new();

    let mut code_blocks = Vec::new();
    for raw in raw_blocks {
        let (parent_id, parent_key) = parent_for_offset(file_id, sections, raw.start);
        let occurrence = next_occurrence(&mut occurrence_counts, &parent_key, CODE_BLOCK_KIND);
        let base_name = if raw.language.is_empty() {
            "code block".to_string()
        } else {
            format!("code block: {}", raw.language)
        };
        let name = unique_name(&mut name_counts, parent_id, CODE_BLOCK_KIND, &base_name);
        code_blocks.push(CodeBlock {
            id: code_block_id(path, &parent_key, occurrence),
            parent_id,
            name,
            body: slice(content, raw.start, raw.end),
            info: raw.info,
            language: raw.language,
        });
    }

    let mut links = Vec::new();
    for raw in raw_links {
        let kind = if raw.image { IMAGE_KIND } else { LINK_KIND };
        let (parent_id, parent_key) = parent_for_offset(file_id, sections, raw.start);
        let occurrence = next_occurrence(&mut occurrence_counts, &parent_key, kind);
        let base_name = link_name(kind, &raw.text, &raw.destination);
        let name = unique_name(&mut name_counts, parent_id, kind, &base_name);
        let id = if raw.image {
            image_id(path, &parent_key, occurrence)
        } else {
            link_id(path, &parent_key, occurrence)
        };
        links.push(Link {
            id,
            parent_id,
            kind: kind.to_string(),
            name,
            body: slice(content, raw.start, raw.end),
            destination: raw.destination,
            title: raw.title,
            text: raw.text,
        });
    }

    (code_blocks, links)
}

fn raw_semantic_children(content: &str) -> (Vec<RawCodeBlock>, Vec<RawLink>) {
    let mut blocks = Vec::new();
    let mut links = Vec::new();
    let mut code_start: Option<(usize, String, String)> = None;
    let mut link_stack: Vec<LinkCapture> = Vec::new();

    for (event, range) in Parser::new(content).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(kind)) => {
                let (info, language) = code_info(kind);
                code_start = Some((range.start, info, language));
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some((start, info, language)) = code_start.take() {
                    blocks.push(RawCodeBlock {
                        start,
                        end: range.end,
                        info,
                        language,
                    });
                }
            }
            Event::Start(Tag::Link {
                dest_url, title, ..
            }) => {
                link_stack.push(LinkCapture {
                    image: false,
                    start: range.start,
                    destination: dest_url.to_string(),
                    title: title.to_string(),
                    text: String::new(),
                });
            }
            Event::Start(Tag::Image {
                dest_url, title, ..
            }) => {
                link_stack.push(LinkCapture {
                    image: true,
                    start: range.start,
                    destination: dest_url.to_string(),
                    title: title.to_string(),
                    text: String::new(),
                });
            }
            Event::End(TagEnd::Link) => finish_link(&mut link_stack, false, range.end, &mut links),
            Event::End(TagEnd::Image) => finish_link(&mut link_stack, true, range.end, &mut links),
            Event::Text(text) | Event::Code(text) => {
                if let Some(capture) = link_stack.last_mut() {
                    capture.text.push_str(&text);
                }
            }
            _ => {}
        }
    }

    blocks.sort_by_key(|block| block.start);
    links.sort_by_key(|link| link.start);
    (blocks, links)
}

fn finish_link(
    link_stack: &mut Vec<LinkCapture>,
    image: bool,
    end: usize,
    links: &mut Vec<RawLink>,
) {
    let Some(index) = link_stack
        .iter()
        .rposition(|capture| capture.image == image)
    else {
        return;
    };
    let capture = link_stack.remove(index);
    links.push(RawLink {
        image: capture.image,
        start: capture.start,
        end,
        destination: capture.destination,
        title: capture.title,
        text: capture.text.trim().to_string(),
    });
}

fn resolve_destination(
    document: &ParsedDocument,
    link: &Link,
    file_ids: &BTreeMap<String, Uuid>,
    section_ids: &BTreeMap<(String, String), Uuid>,
) -> Option<Uuid> {
    if is_external_destination(&link.destination) {
        return None;
    }

    let (path_part, fragment) = split_destination(&link.destination);
    let target_path = if path_part.is_empty() {
        document.path.clone()
    } else {
        normalize_relative_path(&document.path, &path_part)
    };

    if let Some(fragment) = fragment {
        let anchor = normalize_anchor(&percent_decode(&fragment));
        return section_ids.get(&(target_path, anchor)).copied();
    }

    file_ids.get(&target_path).copied()
}

fn split_destination(destination: &str) -> (String, Option<String>) {
    let trimmed = destination.trim();
    let (path, fragment) = match trimmed.split_once('#') {
        Some((path, fragment)) => (path, Some(fragment.to_string())),
        None => (trimmed, None),
    };
    let path = path.split_once('?').map(|(path, _)| path).unwrap_or(path);
    (path.to_string(), fragment)
}

fn is_external_destination(destination: &str) -> bool {
    let trimmed = destination.trim();
    if trimmed.starts_with("//") {
        return true;
    }
    let scheme_end = trimmed
        .find(|ch| ['/', '#', '?'].contains(&ch))
        .unwrap_or(trimmed.len());
    trimmed[..scheme_end].contains(':')
}

fn normalize_relative_path(current_path: &str, target: &str) -> String {
    let mut parts = if target.starts_with('/') {
        Vec::new()
    } else {
        current_path
            .rsplit_once('/')
            .map(|(dir, _)| dir.split('/').map(str::to_string).collect())
            .unwrap_or_else(Vec::new)
    };

    for part in target.trim_start_matches('/').split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other.to_string()),
        }
    }

    parts.join("/")
}

fn parent_for_offset(file_id: Uuid, sections: &[Section], offset: usize) -> (Uuid, String) {
    sections
        .iter()
        .filter(|section| offset >= section.start && offset < section.end)
        .max_by(|left, right| {
            left.level
                .cmp(&right.level)
                .then_with(|| left.start.cmp(&right.start))
        })
        .map(|section| (section.id, format!("section:{}", section.identity_path)))
        .unwrap_or_else(|| (file_id, "file".to_string()))
}

fn next_occurrence(
    counts: &mut BTreeMap<(String, String), usize>,
    parent_key: &str,
    kind: &str,
) -> usize {
    let count = counts
        .entry((parent_key.to_string(), kind.to_string()))
        .or_insert(0);
    *count += 1;
    *count
}

fn unique_name(
    counts: &mut BTreeMap<(Uuid, String, String), usize>,
    parent_id: Uuid,
    kind: &str,
    base: &str,
) -> String {
    let base = if base.is_empty() { kind } else { base };
    let count = counts
        .entry((parent_id, kind.to_string(), base.to_string()))
        .or_insert(0);
    *count += 1;
    if *count == 1 {
        base.to_string()
    } else {
        format!("{base} ({count})")
    }
}

fn link_name(kind: &str, text: &str, destination: &str) -> String {
    let label = compact_label(text);
    if label.is_empty() {
        format!("{kind}: {destination}")
    } else {
        format!("{kind}: {label}")
    }
}

fn compact_label(text: &str) -> String {
    let mut compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.len() > 64 {
        compact.truncate(61);
        compact.push_str("...");
    }
    compact
}

fn code_info(kind: CodeBlockKind<'_>) -> (String, String) {
    match kind {
        CodeBlockKind::Fenced(info) => {
            let info = info.to_string();
            let language = info.split_whitespace().next().unwrap_or("").to_string();
            (info, language)
        }
        CodeBlockKind::Indented => (String::new(), String::new()),
    }
}

fn heading_level(level: HeadingLevel) -> usize {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn slug(text: &str) -> String {
    let mut slug = String::new();
    let mut needs_dash = false;

    for ch in text.chars() {
        if ch.is_alphanumeric() {
            if needs_dash && !slug.is_empty() {
                slug.push('-');
            }
            for lower in ch.to_lowercase() {
                slug.push(lower);
            }
            needs_dash = false;
        } else if ch.is_whitespace() || ch == '-' {
            needs_dash = true;
        }
    }

    if slug.is_empty() {
        "section".to_string()
    } else {
        slug
    }
}

fn normalize_anchor(anchor: &str) -> String {
    slug(anchor.trim_start_matches('#'))
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) = (hex(bytes[index + 1]), hex(bytes[index + 2]))
        {
            decoded.push(high * 16 + low);
            index += 3;
            continue;
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn slice(content: &str, start: usize, end: usize) -> String {
    if start <= end
        && end <= content.len()
        && content.is_char_boundary(start)
        && content.is_char_boundary(end)
    {
        content[start..end].to_string()
    } else {
        String::new()
    }
}

fn trim_line_ending(line: &str) -> &str {
    line.trim_end_matches(['\r', '\n'])
}
