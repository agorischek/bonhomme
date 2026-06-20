use uuid::Uuid;

pub(crate) fn stable_import_uuid(seed: &str) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("https://bonhomme.local/import/{seed}").as_bytes(),
    )
}
