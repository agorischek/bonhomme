use super::base::{BaseFunction, BaseMethod};
use crate::parse::{ParsedFunction, ParsedMethod};
use anyhow::{Result, bail};
use std::collections::BTreeSet;

const RENAME_SIMILARITY_THRESHOLD: f64 = 0.60;
const RENAME_SIMILARITY_MARGIN: f64 = 0.20;

pub(super) trait SymbolLike {
    fn name(&self) -> &str;
    fn body(&self) -> &str;
}

pub(super) trait EditedLike {
    fn name(&self) -> &str;
    fn body(&self) -> &str;
}

impl SymbolLike for BaseFunction {
    fn name(&self) -> &str {
        &self.name
    }

    fn body(&self) -> &str {
        &self.body
    }
}

impl SymbolLike for BaseMethod {
    fn name(&self) -> &str {
        &self.name
    }

    fn body(&self) -> &str {
        &self.body
    }
}

impl EditedLike for ParsedFunction {
    fn name(&self) -> &str {
        &self.name
    }

    fn body(&self) -> &str {
        &self.body
    }
}

impl EditedLike for ParsedMethod {
    fn name(&self) -> &str {
        &self.name
    }

    fn body(&self) -> &str {
        &self.body
    }
}

#[derive(Clone, Debug)]
pub(super) struct MatchPlan {
    pub(super) matched: Vec<(usize, usize)>,
    pub(super) added: Vec<usize>,
    pub(super) deleted: Vec<usize>,
}

pub(super) fn match_container<B, E>(base: &[B], edited: &[E], kind: &str) -> Result<MatchPlan>
where
    B: SymbolLike,
    E: EditedLike,
{
    let mut matched = Vec::new();
    let mut consumed_base = BTreeSet::new();
    let mut consumed_edited = BTreeSet::new();

    match_by_name(
        base,
        edited,
        &mut consumed_base,
        &mut consumed_edited,
        &mut matched,
    );
    match_by_similarity(
        base,
        edited,
        kind,
        &mut consumed_base,
        &mut consumed_edited,
        &mut matched,
    )?;

    matched.sort_unstable();
    Ok(MatchPlan {
        matched,
        added: unconsumed_indexes(edited.len(), &consumed_edited),
        deleted: unconsumed_indexes(base.len(), &consumed_base),
    })
}

fn match_by_name<B, E>(
    base: &[B],
    edited: &[E],
    consumed_base: &mut BTreeSet<usize>,
    consumed_edited: &mut BTreeSet<usize>,
    matched: &mut Vec<(usize, usize)>,
) where
    B: SymbolLike,
    E: EditedLike,
{
    for (edited_index, edited_symbol) in edited.iter().enumerate() {
        if let Some((base_index, _)) = base.iter().enumerate().find(|(base_index, base_symbol)| {
            !consumed_base.contains(base_index) && base_symbol.name() == edited_symbol.name()
        }) {
            consumed_base.insert(base_index);
            consumed_edited.insert(edited_index);
            matched.push((base_index, edited_index));
        }
    }
}

fn match_by_similarity<B, E>(
    base: &[B],
    edited: &[E],
    kind: &str,
    consumed_base: &mut BTreeSet<usize>,
    consumed_edited: &mut BTreeSet<usize>,
    matched: &mut Vec<(usize, usize)>,
) -> Result<()>
where
    B: SymbolLike,
    E: EditedLike,
{
    let mut unmatched_base = unconsumed_indexes(base.len(), consumed_base);
    let mut unmatched_edited = unconsumed_indexes(edited.len(), consumed_edited);
    if unmatched_base.len() == 1 && unmatched_edited.len() == 1 {
        match_single_similarity(
            base,
            edited,
            &unmatched_base,
            &unmatched_edited,
            consumed_base,
            consumed_edited,
            matched,
        );
        return Ok(());
    }

    if unmatched_base.is_empty() || unmatched_edited.is_empty() {
        return Ok(());
    }

    while let Some((base_index, edited_index)) =
        best_similarity_match(base, edited, &unmatched_base, &unmatched_edited)
    {
        consumed_base.insert(base_index);
        consumed_edited.insert(edited_index);
        matched.push((base_index, edited_index));
        unmatched_base = unconsumed_indexes(base.len(), consumed_base);
        unmatched_edited = unconsumed_indexes(edited.len(), consumed_edited);
    }

    if !unmatched_base.is_empty() && !unmatched_edited.is_empty() {
        bail!(
            "ambiguous structural {kind} identity recovery: {} existing and {} edited symbols remain unmatched",
            unmatched_base.len(),
            unmatched_edited.len()
        );
    }
    Ok(())
}

fn match_single_similarity<B, E>(
    base: &[B],
    edited: &[E],
    unmatched_base: &[usize],
    unmatched_edited: &[usize],
    consumed_base: &mut BTreeSet<usize>,
    consumed_edited: &mut BTreeSet<usize>,
    matched: &mut Vec<(usize, usize)>,
) where
    B: SymbolLike,
    E: EditedLike,
{
    let base_index = unmatched_base[0];
    let edited_index = unmatched_edited[0];
    if body_similarity(base[base_index].body(), edited[edited_index].body())
        >= RENAME_SIMILARITY_THRESHOLD
    {
        consumed_base.insert(base_index);
        consumed_edited.insert(edited_index);
        matched.push((base_index, edited_index));
    }
}

fn best_similarity_match<B, E>(
    base: &[B],
    edited: &[E],
    unmatched_base: &[usize],
    unmatched_edited: &[usize],
) -> Option<(usize, usize)>
where
    B: SymbolLike,
    E: EditedLike,
{
    let mut scored = Vec::new();
    for base_index in unmatched_base {
        for edited_index in unmatched_edited {
            scored.push((
                body_similarity(base[*base_index].body(), edited[*edited_index].body()),
                *base_index,
                *edited_index,
            ));
        }
    }
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    let (best_score, best_base, best_edited) = *scored.first()?;
    if best_score < RENAME_SIMILARITY_THRESHOLD {
        return None;
    }
    let second_score = scored.get(1).map_or(0.0, |score| score.0);
    (best_score - second_score >= RENAME_SIMILARITY_MARGIN).then_some((best_base, best_edited))
}

fn body_similarity(left: &str, right: &str) -> f64 {
    let left = normalized_tokens(left);
    let right = normalized_tokens(right);
    if left.is_empty() && right.is_empty() {
        return 1.0;
    }
    let intersection = left.intersection(&right).count();
    let union = left.union(&right).count();
    intersection as f64 / union as f64
}

fn normalized_tokens(value: &str) -> BTreeSet<String> {
    value
        .split(|char: char| !(char.is_alphanumeric() || char == '_'))
        .filter(|token| !token.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

fn unconsumed_indexes(len: usize, consumed: &BTreeSet<usize>) -> Vec<usize> {
    (0..len).filter(|index| !consumed.contains(index)).collect()
}
