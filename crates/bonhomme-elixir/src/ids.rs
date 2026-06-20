use uuid::Uuid;

pub(crate) fn stable_elixir_uuid(seed: &str) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("https://bonhomme.local/elixir/{seed}").as_bytes(),
    )
}

pub(crate) fn file_id(path: &str) -> Uuid {
    stable_elixir_uuid(&format!("file:{path}"))
}

pub(crate) fn module_id(parent_id: Uuid, name: &str) -> Uuid {
    stable_elixir_uuid(&format!("module:{parent_id}:{name}"))
}

pub(crate) fn function_id(parent_id: Uuid, name: &str, arity: usize, visibility: &str) -> Uuid {
    stable_elixir_uuid(&format!("function:{parent_id}:{visibility}:{name}/{arity}"))
}

pub(crate) fn reference_id(from_symbol_id: Uuid, to_symbol_id: Uuid, kind: &str) -> Uuid {
    stable_elixir_uuid(&format!("reference:{from_symbol_id}:{to_symbol_id}:{kind}"))
}
