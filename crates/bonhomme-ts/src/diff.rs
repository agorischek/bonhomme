use crate::{
    import::create_operations_from_parsed_files,
    parse::{ParsedFile, parse_files},
    recover::{diff_graph_parsed_files, ensure_unique_symbol_ids},
};
use anyhow::Result;
use bonhomme_core::{Operation, RenderedFile, SemanticGraph};
use std::collections::BTreeMap;
use uuid::Uuid;

pub fn diff_slice(original: &[RenderedFile], modified: &[RenderedFile]) -> Result<Vec<Operation>> {
    let original_by_path = parse_files(original)?;
    let modified_by_path = parse_files(modified)?;
    ensure_unique_symbol_ids(&modified_by_path)?;

    let base = materialize_parsed_files(&original_by_path)?;
    diff_graph_parsed_files(&base, &[], modified_by_path)
}

fn materialize_parsed_files(files: &BTreeMap<String, ParsedFile>) -> Result<SemanticGraph> {
    let parsed_files = files.values().cloned().collect::<Vec<_>>();
    let operations = create_operations_from_parsed_files(&parsed_files);
    let mut graph = SemanticGraph::default();
    for (index, operation) in operations.iter().enumerate() {
        graph.apply_operation(Uuid::from_u128(index as u128 + 1), operation)?;
    }
    graph.validate()?;
    Ok(graph)
}
