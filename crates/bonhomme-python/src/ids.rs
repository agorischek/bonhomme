use uuid::Uuid;

pub(crate) fn stable_python_uuid(seed: &str) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("https://bonhomme.local/python/{seed}").as_bytes(),
    )
}

pub(crate) fn file_id(path: &str) -> Uuid {
    stable_python_uuid(&format!("file:{path}"))
}

pub(crate) fn class_id(path: &str, name: &str) -> Uuid {
    stable_python_uuid(&format!("class:{path}:{name}"))
}

pub(crate) fn function_id(path: &str, name: &str) -> Uuid {
    stable_python_uuid(&format!("function:{path}:{name}"))
}

pub(crate) fn method_id(parent_id: Uuid, name: &str) -> Uuid {
    stable_python_uuid(&format!("method:{parent_id}:{name}"))
}

pub(crate) fn value_id(path: &str, name: &str) -> Uuid {
    stable_python_uuid(&format!("value:{path}:{name}"))
}

pub(crate) fn attribute_id(parent_id: Uuid, name: &str) -> Uuid {
    stable_python_uuid(&format!("attribute:{parent_id}:{name}"))
}

pub(crate) fn reference_id(from_symbol_id: Uuid, to_symbol_id: Uuid, kind: &str) -> Uuid {
    stable_python_uuid(&format!("reference:{from_symbol_id}:{to_symbol_id}:{kind}"))
}
