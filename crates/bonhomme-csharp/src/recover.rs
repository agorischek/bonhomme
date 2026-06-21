use crate::import::import_csharp_files;
use anyhow::Result;
use bonhomme_core::{
    DesiredRecoveryOptions, Operation, RenderedFile, SemanticGraph, recover_from_desired_operations,
};
use std::collections::BTreeSet;
use uuid::Uuid;

pub fn recover_csharp_operations(
    base: &SemanticGraph,
    scope: &[Uuid],
    edited: &[RenderedFile],
) -> Result<Vec<Operation>> {
    let desired_ops = import_csharp_files(edited)?;
    recover_from_desired_operations(
        base,
        scope,
        &edited_paths(edited),
        &desired_ops,
        DesiredRecoveryOptions::all_missing_references(),
    )
}

fn edited_paths(edited: &[RenderedFile]) -> BTreeSet<String> {
    edited.iter().map(|file| file.path.clone()).collect()
}
