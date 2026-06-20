use serde_json::Value;

pub fn metadata_string(metadata: &Value, key: &str) -> Option<String> {
    metadata
        .get(key)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

pub fn metadata_bool(metadata: &Value, key: &str) -> bool {
    metadata.get(key).and_then(Value::as_bool).unwrap_or(false)
}
