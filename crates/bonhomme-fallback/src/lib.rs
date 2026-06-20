//! Fallback handlers for files without a full language plugin. The fallback is not an edge case but
//! the floor: every file resolves to *some* handler, and language plugins are progressive
//! enhancements layered on top. The universal floor — [`bonhomme_core::BlobHandler`] — lives in
//! core next to the [`bonhomme_core::HandlerRegistry`]; this crate adds the higher tiers that pull
//! their own parsers: structured-data handlers ([`JsonHandler`], [`MarkdownHandler`]) and the
//! tree-sitter structural-lite tier.
//!
//! Every tier merges and conflicts at whole-unit granularity and never auto-resolves *inside* a
//! unit — a handler may make units finer (file → key/section) but never line-merges within a unit,
//! because that would reintroduce the text merge bonhomme exists to transcend, with no validator to
//! catch a silently-wrong result.

mod ids;
mod json;
mod markdown;
mod toml;
mod treesitter;
mod yaml;

pub use bonhomme_core::BlobHandler;
pub use json::JsonHandler;
pub use markdown::MarkdownHandler;
pub use toml::TomlHandler;
pub use treesitter::TreeSitterHandler;
pub use yaml::YamlHandler;

#[cfg(test)]
mod test_support {
    use bonhomme_core::{Operation, OperationRecord, SemanticGraph, materialize};
    use uuid::Uuid;

    /// Materialize bare operations into a graph for handler tests. Record metadata (ids, position,
    /// timestamp) is irrelevant to replay beyond uniqueness.
    pub(crate) fn graph_from(operations: &[Operation]) -> SemanticGraph {
        let records = operations
            .iter()
            .enumerate()
            .map(|(index, operation)| OperationRecord {
                id: Uuid::new_v4(),
                repository_id: Uuid::nil(),
                branch_id: Uuid::nil(),
                changeset_id: Uuid::nil(),
                position: index as i64 + 1,
                operation: operation.clone(),
                created_at: chrono::DateTime::<chrono::Utc>::UNIX_EPOCH,
            })
            .collect::<Vec<_>>();
        materialize(&records).expect("operations materialize")
    }
}
