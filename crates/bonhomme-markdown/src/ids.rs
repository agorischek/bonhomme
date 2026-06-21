use uuid::Uuid;

pub(crate) fn file_id(path: &str) -> Uuid {
    stable_uuid(&format!("markdown:file:{path}"))
}

pub(crate) fn frontmatter_id(path: &str) -> Uuid {
    stable_uuid(&format!("markdown:frontmatter:{path}"))
}

pub(crate) fn section_id(path: &str, identity_path: &str) -> Uuid {
    stable_uuid(&format!("markdown:section:{path}:{identity_path}"))
}

pub(crate) fn code_block_id(path: &str, parent_key: &str, occurrence: usize) -> Uuid {
    stable_uuid(&format!(
        "markdown:code-block:{path}:{parent_key}:{occurrence}"
    ))
}

pub(crate) fn link_id(path: &str, parent_key: &str, occurrence: usize) -> Uuid {
    stable_uuid(&format!("markdown:link:{path}:{parent_key}:{occurrence}"))
}

pub(crate) fn image_id(path: &str, parent_key: &str, occurrence: usize) -> Uuid {
    stable_uuid(&format!("markdown:image:{path}:{parent_key}:{occurrence}"))
}

pub(crate) fn reference_id(from_symbol_id: Uuid, to_symbol_id: Uuid, kind: &str) -> Uuid {
    stable_uuid(&format!(
        "markdown:reference:{from_symbol_id}:{to_symbol_id}:{kind}"
    ))
}

fn stable_uuid(seed: &str) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("https://bonhomme.local/{seed}").as_bytes(),
    )
}
