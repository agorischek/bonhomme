use uuid::Uuid;

pub(crate) fn stable_php_uuid(seed: &str) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("https://bonhomme.local/php/{seed}").as_bytes(),
    )
}

pub(crate) fn file_id(path: &str) -> Uuid {
    stable_php_uuid(&format!("file:{path}"))
}

pub(crate) fn type_id(path: &str, namespace: Option<&str>, kind: &str, name: &str) -> Uuid {
    stable_php_uuid(&format!(
        "type:{path}:{}:{kind}:{name}",
        namespace.unwrap_or("")
    ))
}

pub(crate) fn function_id(path: &str, namespace: Option<&str>, identity: &str) -> Uuid {
    stable_php_uuid(&format!(
        "function:{path}:{}:{identity}",
        namespace.unwrap_or("")
    ))
}

pub(crate) fn member_id(parent_id: Uuid, kind: &str, identity: &str) -> Uuid {
    stable_php_uuid(&format!("member:{parent_id}:{kind}:{identity}"))
}

pub(crate) fn reference_id(from_symbol_id: Uuid, to_symbol_id: Uuid, kind: &str) -> Uuid {
    stable_php_uuid(&format!("reference:{from_symbol_id}:{to_symbol_id}:{kind}"))
}
