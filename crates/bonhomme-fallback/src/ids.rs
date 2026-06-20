use uuid::Uuid;

/// A deterministic v5 UUID for a fallback symbol, so re-importing the same file yields the same
/// identity (the structural-identity model's "the path / key-path / heading-path is the identity").
/// Callers prefix the `seed` per handler and kind to keep namespaces from colliding.
pub(crate) fn stable_uuid(seed: &str) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("https://bonhomme.local/fallback/{seed}").as_bytes(),
    )
}
