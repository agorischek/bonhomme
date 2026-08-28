use uuid::Uuid;

pub(crate) fn stable_swift_uuid(seed: &str) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("https://bonhomme.local/swift/{seed}").as_bytes(),
    )
}

pub(crate) fn file_id(path: &str) -> Uuid {
    stable_swift_uuid(&format!("file:{path}"))
}

pub(crate) fn type_id(path: &str, parent_id: Option<Uuid>, kind: &str, name: &str) -> Uuid {
    stable_swift_uuid(&format!(
        "type:{path}:{}:{kind}:{name}",
        parent_id.map(|id| id.to_string()).unwrap_or_default()
    ))
}

pub(crate) fn member_id(parent_id: Uuid, kind: &str, identity: &str) -> Uuid {
    stable_swift_uuid(&format!("member:{parent_id}:{kind}:{identity}"))
}

pub(crate) fn reference_id(from_symbol_id: Uuid, to_symbol_id: Uuid, kind: &str) -> Uuid {
    stable_swift_uuid(&format!("reference:{from_symbol_id}:{to_symbol_id}:{kind}"))
}
