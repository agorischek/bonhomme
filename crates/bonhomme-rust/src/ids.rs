use uuid::Uuid;

pub(crate) fn stable_rust_uuid(seed: &str) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("https://bonhomme.local/rust/{seed}").as_bytes(),
    )
}

pub(crate) fn file_id(path: &str) -> Uuid {
    stable_rust_uuid(&format!("file:{path}"))
}

pub(crate) fn type_id(path: &str, name: &str) -> Uuid {
    stable_rust_uuid(&format!("type:{path}:{name}"))
}

pub(crate) fn function_id(path: &str, name: &str) -> Uuid {
    stable_rust_uuid(&format!("function:{path}:{name}"))
}

pub(crate) fn value_id(path: &str, kind: &str, name: &str) -> Uuid {
    stable_rust_uuid(&format!("value:{path}:{kind}:{name}"))
}

pub(crate) fn field_id(parent_id: Uuid, name: &str) -> Uuid {
    stable_rust_uuid(&format!("field:{parent_id}:{name}"))
}

pub(crate) fn variant_id(parent_id: Uuid, name: &str) -> Uuid {
    stable_rust_uuid(&format!("variant:{parent_id}:{name}"))
}

pub(crate) fn trait_method_id(parent_id: Uuid, name: &str) -> Uuid {
    stable_rust_uuid(&format!("trait-method:{parent_id}:{name}"))
}

pub(crate) fn impl_id(path: &str, header: &str) -> Uuid {
    stable_rust_uuid(&format!("impl:{path}:{header}"))
}

pub(crate) fn method_id(parent_id: Uuid, impl_header: &str, name: &str) -> Uuid {
    stable_rust_uuid(&format!("method:{parent_id}:{impl_header}:{name}"))
}

pub(crate) fn reference_id(from_symbol_id: Uuid, to_symbol_id: Uuid, kind: &str) -> Uuid {
    stable_rust_uuid(&format!("reference:{from_symbol_id}:{to_symbol_id}:{kind}"))
}
