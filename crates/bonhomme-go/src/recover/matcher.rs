use super::base::BaseSymbol;
use crate::model::{Declaration, Field, InterfaceMethod};
use anyhow::{Result, bail};
use std::collections::BTreeSet;

pub(super) struct MatchPlan {
    pub(super) matched: Vec<(usize, usize)>,
    pub(super) added: Vec<usize>,
    pub(super) deleted: Vec<usize>,
}

pub(super) trait Named {
    fn name(&self) -> &str;
    fn body(&self) -> &str {
        ""
    }
}

impl Named for BaseSymbol {
    fn name(&self) -> &str {
        &self.name
    }

    fn body(&self) -> &str {
        &self.body
    }
}

impl Named for &Declaration {
    fn name(&self) -> &str {
        &self.name
    }

    fn body(&self) -> &str {
        self.body.as_deref().unwrap_or("")
    }
}

impl Named for &Field {
    fn name(&self) -> &str {
        &self.name
    }
}

impl Named for &InterfaceMethod {
    fn name(&self) -> &str {
        &self.name
    }
}

pub(super) fn match_by_name<B, E>(base: &[B], edited: &[E]) -> MatchPlan
where
    B: Named,
    E: Named,
{
    let mut matched = Vec::new();
    let mut consumed_base = BTreeSet::new();
    let mut consumed_edited = BTreeSet::new();
    for (edited_index, edited_symbol) in edited.iter().enumerate() {
        if let Some((base_index, _)) = base.iter().enumerate().find(|(base_index, base_symbol)| {
            !consumed_base.contains(base_index) && base_symbol.name() == edited_symbol.name()
        }) {
            consumed_base.insert(base_index);
            consumed_edited.insert(edited_index);
            matched.push((base_index, edited_index));
        }
    }
    MatchPlan {
        matched,
        added: unconsumed(edited.len(), &consumed_edited),
        deleted: unconsumed(base.len(), &consumed_base),
    }
}

pub(super) fn match_by_body<B, E>(base: &[B], edited: &[E], container: &str) -> Result<MatchPlan>
where
    B: Named,
    E: Named,
{
    let mut plan = match_by_name(base, edited);
    if plan.added.len() == 1 && plan.deleted.len() == 1 {
        let base_index = plan.deleted[0];
        let edited_index = plan.added[0];
        if body_similarity(base[base_index].body(), edited[edited_index].body()) >= 0.60 {
            plan.matched.push((base_index, edited_index));
            plan.deleted.clear();
            plan.added.clear();
        }
    } else if !plan.added.is_empty() && !plan.deleted.is_empty() {
        bail!("ambiguous structural Go identity recovery in {container}; refusing to guess");
    }
    Ok(plan)
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

fn unconsumed(len: usize, consumed: &BTreeSet<usize>) -> Vec<usize> {
    (0..len).filter(|index| !consumed.contains(index)).collect()
}
