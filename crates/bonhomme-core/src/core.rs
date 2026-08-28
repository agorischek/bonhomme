mod desired_recovery;
mod graph;
mod merge;
mod metadata;
mod models;
mod operation;
mod protocol;
mod queries;

#[cfg(test)]
mod tests;

pub use desired_recovery::{
    DesiredRecoveryOptions, recover_from_desired_operations, scoped_file_symbols_by_path,
};
pub use graph::{SemanticGraph, detect_moves, materialize};
pub use merge::{MergeAnalysis, MergeConflict, MergeOutcome, analyze_merge};
pub use metadata::{metadata_bool, metadata_string};
pub use models::{Branch, ChangeSet, OperationRecord, ReferenceNode, Repository, SymbolNode, Task};
pub use operation::{
    Operation, OperationCondition, OperationContract, OperationEffect, SymbolNameKey,
};
pub use protocol::{
    CURRENT_PROTOCOL_VERSION, DetachedSignature, GraphSnapshot, LegacyBatchContext, OperationBatch,
    ProtocolProvenance, ProtocolRange, ProtocolVersion, SUPPORTED_PROTOCOL_RANGE, SymbolIdentityV1,
    canonical_json_bytes, migrate_legacy_operations,
};
