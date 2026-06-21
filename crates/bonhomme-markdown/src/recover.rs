use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use bonhomme_core::{
    DesiredRecoveryOptions, Operation, RenderedFile, SemanticGraph, SymbolNode,
    recover_from_desired_operations, scoped_file_symbols_by_path,
};
use uuid::Uuid;

use crate::{import::import_markdown_files, render::render_files};

pub fn recover_markdown_operations(
    base: &SemanticGraph,
    scope: &[Uuid],
    edited: &[RenderedFile],
) -> Result<Vec<Operation>> {
    let base_files = scoped_file_symbols_by_path(base, scope);
    let desired_files = desired_files(base, &base_files, scope, edited);
    let desired_ops = import_markdown_files(&desired_files)?;

    recover_from_desired_operations(
        base,
        scope,
        &edited_paths(edited),
        &desired_ops,
        recovery_options(scope, edited),
    )
}

fn desired_files(
    base: &SemanticGraph,
    base_files: &BTreeMap<String, &SymbolNode>,
    scope: &[Uuid],
    edited: &[RenderedFile],
) -> Vec<RenderedFile> {
    if scope.is_empty() {
        return edited.to_vec();
    }

    let edited_paths = edited
        .iter()
        .map(|file| file.path.as_str())
        .collect::<BTreeSet<_>>();
    let scoped_paths = base_files
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let mut files = edited.to_vec();

    for file in render_files(base) {
        if !edited_paths.contains(file.path.as_str()) && !scoped_paths.contains(file.path.as_str())
        {
            files.push(file);
        }
    }

    files
}

fn recovery_options(scope: &[Uuid], edited: &[RenderedFile]) -> DesiredRecoveryOptions {
    if scope.is_empty() {
        return DesiredRecoveryOptions::all_missing_references();
    }
    DesiredRecoveryOptions::scoped_references(edited_paths(edited))
}

fn edited_paths(edited: &[RenderedFile]) -> BTreeSet<String> {
    edited.iter().map(|file| file.path.clone()).collect()
}
