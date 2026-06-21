mod recovery;
mod source;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use anyhow::Result;
use uuid::Uuid;

use super::{LanguagePlugin, RenderedFile, Slice, ValidateFuture};
use crate::core::{Operation, ReferenceNode, SemanticGraph, SymbolNode, metadata_string};

pub use source::read_source_files;

const MAX_DEGRADATION_REPORTS: usize = 20;

/// A [`LanguagePlugin`] that additionally declares which files it claims and a stable name used to
/// tag the file symbols it produces. The fallback model treats *every* file handler — including the
/// opaque blob floor — as a `Handler`; a language plugin is simply a handler that claims narrowly.
///
/// Resolution is per-file: the [`HandlerRegistry`] maps each file to the first handler that claims
/// it, with a terminal handler (the blob handler) claiming everything as the universal floor.
pub trait Handler: LanguagePlugin {
    /// Stable identifier stored in each file symbol's `handler` metadata tag and used to
    /// re-dispatch render/recover/validate by file. e.g. `"typescript"`, `"json"`, `"blob"`.
    fn name(&self) -> &str;

    /// Whether this handler claims the file, by extension first and content sniff as a tiebreak.
    /// The registry resolves a file to the first claimant in registry order; the terminal handler
    /// claims everything.
    fn claims(&self, file: &RenderedFile) -> bool;
}

/// The per-file router. An ordered set of [`Handler`]s that *is itself* a [`LanguagePlugin`], so the
/// storage/merge engine holds one `Arc<dyn LanguagePlugin>` and never grows a "no plugin" branch.
/// Import dispatches by [`Handler::claims`]; render/recover/validate re-dispatch by the `handler`
/// metadata tag stored on each file symbol (falling back to claims for untagged legacy symbols).
///
/// Every method partitions the work by handler and concatenates the results. The merge engine is
/// untouched: each handler emits ordinary symbol operations, so two branches editing different
/// files touch different symbols (clean) and the same file touches one symbol (conflict).
pub struct HandlerRegistry {
    handlers: Vec<Arc<dyn Handler>>,
    /// When a non-terminal handler fails to import a file, degrade just that file to the terminal
    /// (blob) handler with a warning rather than failing the whole import. One broken file should
    /// not sink the repo. Defaults to `true`; set false to reject instead.
    degrade_on_error: bool,
}

impl HandlerRegistry {
    /// Build a registry from handlers in priority order. The final handler must be terminal (claim
    /// everything) so resolution always succeeds; conventionally the blob handler.
    pub fn new(handlers: Vec<Arc<dyn Handler>>) -> Self {
        assert!(
            !handlers.is_empty(),
            "a HandlerRegistry needs at least one (terminal) handler"
        );
        Self {
            handlers,
            degrade_on_error: true,
        }
    }

    /// Reject (propagate the error) instead of degrading an unparseable file to a blob.
    pub fn rejecting_parse_errors(mut self) -> Self {
        self.degrade_on_error = false;
        self
    }

    /// Count root file symbols by the handler that owns them — the transparency breakdown a caller
    /// surfaces as "5 files merged semantically, 3 as opaque blobs". A file degraded to blob shows
    /// up here as `blob`, so degradation is visible, never silent.
    pub fn handler_breakdown(&self, graph: &SemanticGraph) -> BTreeMap<String, usize> {
        let mut counts = BTreeMap::new();
        for file_symbol in file_symbols(graph) {
            let handler = self.handler_for_symbol(file_symbol).name().to_string();
            *counts.entry(handler).or_insert(0) += 1;
        }
        counts
    }

    /// The handler names in priority order — used for transparency reporting.
    pub fn handler_names(&self) -> Vec<&str> {
        self.handlers.iter().map(|handler| handler.name()).collect()
    }

    /// The first handler that claims `file`, falling back to the terminal (last) handler so this
    /// never fails even if no handler explicitly claims.
    fn claimant(&self, file: &RenderedFile) -> &Arc<dyn Handler> {
        self.handlers
            .iter()
            .find(|handler| handler.claims(file))
            .unwrap_or_else(|| self.terminal())
    }

    fn terminal(&self) -> &Arc<dyn Handler> {
        self.handlers.last().expect("registry is non-empty")
    }

    fn by_name(&self, name: &str) -> Option<&Arc<dyn Handler>> {
        self.handlers.iter().find(|handler| handler.name() == name)
    }

    /// Resolve the handler that owns a file symbol: its stored `handler` tag if present, else
    /// re-derived by claims on the path. The fallback keeps file symbols imported before tagging
    /// existed (demo seeds, older repos) rendering correctly.
    fn handler_for_symbol(&self, symbol: &SymbolNode) -> &Arc<dyn Handler> {
        if let Some(tag) = metadata_string(&symbol.metadata, "handler")
            && let Some(handler) = self.by_name(&tag)
        {
            return handler;
        }
        let path = file_symbol_path(symbol);
        self.claimant(&RenderedFile {
            path,
            content: String::new(),
        })
    }

    /// Group file symbols by the index of the handler that owns them, preserving registry order so
    /// the output is deterministic.
    fn group_file_symbols<'a>(
        &self,
        graph: &'a SemanticGraph,
    ) -> Vec<(usize, Vec<&'a SymbolNode>)> {
        let mut groups: BTreeMap<usize, Vec<&SymbolNode>> = BTreeMap::new();
        for file_symbol in file_symbols(graph) {
            let index = self.handler_index_for_symbol(file_symbol);
            groups.entry(index).or_default().push(file_symbol);
        }
        groups.into_iter().collect()
    }

    fn handler_index_for_symbol(&self, symbol: &SymbolNode) -> usize {
        let chosen = self.handler_for_symbol(symbol);
        self.handlers
            .iter()
            .position(|handler| Arc::ptr_eq(handler, chosen))
            .unwrap_or(self.handlers.len() - 1)
    }
}

impl LanguagePlugin for HandlerRegistry {
    fn render(&self, graph: &SemanticGraph) -> Vec<RenderedFile> {
        let mut files = Vec::new();
        for (index, file_symbols) in self.group_file_symbols(graph) {
            let ids = file_symbols.iter().map(|symbol| symbol.id).collect();
            let subgraph = subgraph_for_files(graph, &ids);
            files.extend(self.handlers[index].render(&subgraph));
        }
        files.sort_by(|a, b| a.path.cmp(&b.path));
        files
    }

    fn render_slice(
        &self,
        graph: &SemanticGraph,
        base_revision: String,
        root_symbols: Vec<Uuid>,
    ) -> Slice {
        let mut files = Vec::new();

        if root_symbols.is_empty() {
            // Whole-repo slice: each handler renders all of its files.
            for (index, file_symbols) in self.group_file_symbols(graph) {
                let ids = file_symbols.iter().map(|symbol| symbol.id).collect();
                let subgraph = subgraph_for_files(graph, &ids);
                files.extend(
                    self.handlers[index]
                        .render_slice(&subgraph, base_revision.clone(), Vec::new())
                        .files,
                );
            }
        } else {
            // Group requested roots by the handler owning their nearest file symbol.
            let mut groups: BTreeMap<usize, (BTreeSet<Uuid>, Vec<Uuid>)> = BTreeMap::new();
            for root in &root_symbols {
                let Some(symbol) = graph.symbols.get(root) else {
                    continue;
                };
                let Some(file_symbol) = nearest_file_symbol(graph, symbol) else {
                    continue;
                };
                let index = self.handler_index_for_symbol(file_symbol);
                let entry = groups.entry(index).or_default();
                entry.0.insert(file_symbol.id);
                entry.1.push(*root);
            }
            for (index, (file_ids, roots)) in groups {
                let subgraph = subgraph_for_files(graph, &file_ids);
                files.extend(
                    self.handlers[index]
                        .render_slice(&subgraph, base_revision.clone(), roots)
                        .files,
                );
            }
        }

        files.sort_by(|a, b| a.path.cmp(&b.path));
        files.dedup_by(|a, b| a.path == b.path);

        Slice {
            id: Uuid::new_v4(),
            base_revision,
            root_symbols,
            files,
        }
    }

    fn import(&self, files: &[RenderedFile]) -> Result<Vec<Operation>> {
        let mut operations = Vec::new();
        let mut degraded: Vec<(String, String)> = Vec::new();
        for (index, subset) in self.partition_by_claims(files) {
            let (ops, degraded_paths) = self.import_subset(index, &subset)?;
            operations.extend(ops);
            let handler = self.handlers[index].name().to_string();
            degraded.extend(
                degraded_paths
                    .into_iter()
                    .map(|path| (handler.clone(), path)),
            );
        }
        if !degraded.is_empty() {
            eprintln!(
                "bonhomme: {} file(s) degraded to opaque blobs after handler parse errors:",
                degraded.len()
            );
            for (handler, path) in degraded.iter().take(MAX_DEGRADATION_REPORTS) {
                eprintln!("  {path} (expected {handler})");
            }
            if degraded.len() > MAX_DEGRADATION_REPORTS {
                eprintln!(
                    "  ... and {} more",
                    degraded.len() - MAX_DEGRADATION_REPORTS
                );
            }
        }
        Ok(operations)
    }

    fn diff(&self, original: &[RenderedFile], modified: &[RenderedFile]) -> Result<Vec<Operation>> {
        // Group by claims over the union of paths so a file present on only one side still routes to
        // a handler (a create or delete).
        let mut original_by_path: BTreeMap<&str, &RenderedFile> = original
            .iter()
            .map(|file| (file.path.as_str(), file))
            .collect();
        let mut modified_by_path: BTreeMap<&str, &RenderedFile> = modified
            .iter()
            .map(|file| (file.path.as_str(), file))
            .collect();

        let mut groups: BTreeMap<usize, (Vec<RenderedFile>, Vec<RenderedFile>)> = BTreeMap::new();
        let mut paths: Vec<&str> = original_by_path
            .keys()
            .chain(modified_by_path.keys())
            .copied()
            .collect();
        paths.sort_unstable();
        paths.dedup();
        for path in paths {
            let probe = modified_by_path
                .get(path)
                .or_else(|| original_by_path.get(path))
                .expect("path came from one of the maps");
            let index = self.claimant_index(probe);
            let entry = groups.entry(index).or_default();
            if let Some(file) = original_by_path.remove(path) {
                entry.0.push(file.clone());
            }
            if let Some(file) = modified_by_path.remove(path) {
                entry.1.push(file.clone());
            }
        }

        let mut operations = Vec::new();
        for (index, (original_subset, modified_subset)) in groups {
            operations.extend(self.handlers[index].diff(&original_subset, &modified_subset)?);
        }
        Ok(operations)
    }

    fn recover_operations(
        &self,
        base: &SemanticGraph,
        scope: &[Uuid],
        edited: &[RenderedFile],
    ) -> Result<Vec<Operation>> {
        self.recover_by_handlers(base, scope, edited)
    }

    fn read_source_tree(&self, root: &std::path::Path) -> Result<Vec<RenderedFile>> {
        read_source_files(root)
    }

    fn validate<'a>(&'a self, files: &'a [RenderedFile]) -> ValidateFuture<'a> {
        // Partition into owned subsets (validate writes them to a temp dir anyway) so each subset
        // outlives the await points inside the returned future.
        let subsets = self.partition_by_claims(files);
        Box::pin(async move {
            for (index, subset) in &subsets {
                self.handlers[*index].validate(subset).await?;
            }
            Ok(())
        })
    }
}

impl HandlerRegistry {
    fn claimant_index(&self, file: &RenderedFile) -> usize {
        let chosen = self.claimant(file);
        self.handlers
            .iter()
            .position(|handler| Arc::ptr_eq(handler, chosen))
            .unwrap_or(self.handlers.len() - 1)
    }

    /// Partition files into owned per-handler subsets keyed by handler index, in registry order.
    fn partition_by_claims(&self, files: &[RenderedFile]) -> Vec<(usize, Vec<RenderedFile>)> {
        let mut groups: BTreeMap<usize, Vec<RenderedFile>> = BTreeMap::new();
        for file in files {
            let index = self.claimant_index(file);
            groups.entry(index).or_default().push(file.clone());
        }
        groups.into_iter().collect()
    }

    fn is_terminal(&self, index: usize) -> bool {
        index == self.handlers.len() - 1
    }

    /// Import one handler's file subset, returning the operations and the paths that had to be
    /// degraded to the blob handler. On a batch import failure (a parse error somewhere), bisect
    /// failed batches to find broken files while keeping the successful files together for the real
    /// import. If the good files still fail together, import the largest successful chunks and
    /// degrade only chunks that cannot be isolated further. The terminal handler never degrades
    /// (there is no floor beneath it), and `rejecting_parse_errors` propagates instead.
    fn import_subset(
        &self,
        index: usize,
        subset: &[RenderedFile],
    ) -> Result<(Vec<Operation>, Vec<String>)> {
        match self.handlers[index].import(subset) {
            Ok(operations) => return Ok((operations, Vec::new())),
            Err(error) => {
                if !self.degrade_on_error || self.is_terminal(index) {
                    return Err(error);
                }
            }
        }

        let mut bad = self.unimportable_files_after_failure(index, subset);
        let bad_paths = bad
            .iter()
            .map(|file| file.path.clone())
            .collect::<BTreeSet<_>>();
        let good = subset
            .iter()
            .filter(|file| !bad_paths.contains(&file.path))
            .cloned()
            .collect::<Vec<_>>();

        let mut operations = Vec::new();
        if !good.is_empty() {
            match self.handlers[index].import(&good) {
                Ok(ops) => operations.extend(ops),
                // The good files parse in smaller groups but not together; keep the largest
                // successful chunks rather than degrading every remaining file.
                Err(_) => {
                    let (ops, extra_bad) =
                        self.import_successful_chunks_after_failure(index, &good)?;
                    operations.extend(ops);
                    bad.extend(extra_bad);
                }
            }
        }

        let degraded_paths = bad.iter().map(|file| file.path.clone()).collect();
        operations.extend(self.terminal().import(&bad)?);
        Ok((operations, degraded_paths))
    }

    fn unimportable_files(&self, index: usize, files: &[RenderedFile]) -> Vec<RenderedFile> {
        if files.is_empty() || self.handlers[index].import(files).is_ok() {
            Vec::new()
        } else {
            self.unimportable_files_after_failure(index, files)
        }
    }

    fn unimportable_files_after_failure(
        &self,
        index: usize,
        files: &[RenderedFile],
    ) -> Vec<RenderedFile> {
        if files.len() <= 1 {
            return files.to_vec();
        }

        let mid = files.len() / 2;
        let mut bad = self.unimportable_files(index, &files[..mid]);
        bad.extend(self.unimportable_files(index, &files[mid..]));
        bad
    }

    fn import_successful_chunks_after_failure(
        &self,
        index: usize,
        files: &[RenderedFile],
    ) -> Result<(Vec<Operation>, Vec<RenderedFile>)> {
        if files.len() <= 1 {
            return Ok((Vec::new(), files.to_vec()));
        }

        let mid = files.len() / 2;
        let (mut left_ops, mut left_bad) = self.import_successful_chunks(index, &files[..mid])?;
        let (right_ops, right_bad) = self.import_successful_chunks(index, &files[mid..])?;
        left_ops.extend(right_ops);
        left_bad.extend(right_bad);
        Ok((left_ops, left_bad))
    }

    fn import_successful_chunks(
        &self,
        index: usize,
        files: &[RenderedFile],
    ) -> Result<(Vec<Operation>, Vec<RenderedFile>)> {
        if files.is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }
        match self.handlers[index].import(files) {
            Ok(operations) => Ok((operations, Vec::new())),
            Err(_) => self.import_successful_chunks_after_failure(index, files),
        }
    }
}

/// Root symbols that are files, in deterministic order.
fn file_symbols(graph: &SemanticGraph) -> Vec<&SymbolNode> {
    graph
        .root_symbols()
        .into_iter()
        .filter(|symbol| symbol.kind == "file")
        .collect()
}

/// A file symbol's path: its `path` metadata, falling back to its name.
fn file_symbol_path(symbol: &SymbolNode) -> String {
    metadata_string(&symbol.metadata, "path").unwrap_or_else(|| symbol.name.clone())
}

/// The nearest enclosing file symbol of `symbol` (itself if it is a file).
fn nearest_file_symbol<'a>(
    graph: &'a SemanticGraph,
    symbol: &'a SymbolNode,
) -> Option<&'a SymbolNode> {
    let mut current = symbol;
    loop {
        if current.kind == "file" {
            return Some(current);
        }
        current = graph.symbols.get(&current.parent_id?)?;
    }
}

/// The id of the root ancestor of `symbol` (the symbol itself if it is a root).
fn root_ancestor(graph: &SemanticGraph, symbol: &SymbolNode) -> Option<Uuid> {
    let mut current = symbol;
    while let Some(parent_id) = current.parent_id {
        current = graph.symbols.get(&parent_id)?;
    }
    Some(current.id)
}

/// Build a subgraph containing exactly the given file symbols and their transitive descendants,
/// plus references whose endpoints are both included. Render and recover only read file subtrees,
/// so this cleanly hands each handler the files it owns without leaking other handlers' symbols.
fn subgraph_for_files(graph: &SemanticGraph, file_ids: &BTreeSet<Uuid>) -> SemanticGraph {
    let mut symbols: BTreeMap<Uuid, SymbolNode> = BTreeMap::new();
    for symbol in graph.symbols.values() {
        if root_ancestor(graph, symbol).is_some_and(|root| file_ids.contains(&root)) {
            symbols.insert(symbol.id, symbol.clone());
        }
    }
    let references: BTreeMap<Uuid, ReferenceNode> = graph
        .references
        .iter()
        .filter(|(_, reference)| {
            symbols.contains_key(&reference.from_symbol_id)
                && symbols.contains_key(&reference.to_symbol_id)
        })
        .map(|(id, reference)| (*id, reference.clone()))
        .collect();
    SemanticGraph {
        symbols,
        references,
        applied_operations: graph.applied_operations.clone(),
    }
}
