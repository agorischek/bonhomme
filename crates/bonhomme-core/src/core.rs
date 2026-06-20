mod graph;
mod merge;
mod metadata;
mod models;
mod operation;
mod queries;

#[cfg(test)]
mod tests;

pub use graph::{SemanticGraph, detect_moves, materialize};
pub use merge::{MergeAnalysis, MergeConflict, MergeOutcome, analyze_merge};
pub use metadata::{metadata_bool, metadata_string};
pub use models::{Branch, ChangeSet, OperationRecord, ReferenceNode, Repository, SymbolNode, Task};
pub use operation::{Operation, SymbolNameKey};
