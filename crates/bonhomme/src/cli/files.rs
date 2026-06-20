use std::path::Path;

use anyhow::Result;
use bonhomme_core::RenderedFile;
use tokio::fs;

pub(super) async fn read_rendered_files(path: &Path) -> Result<Vec<RenderedFile>> {
    let raw = fs::read_to_string(path).await?;
    if let Ok(files) = serde_json::from_str::<Vec<RenderedFile>>(&raw) {
        return Ok(files);
    }

    if let Ok(file) = serde_json::from_str::<RenderedFile>(&raw) {
        return Ok(vec![file]);
    }

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("slice.ts")
        .to_string();
    Ok(vec![RenderedFile {
        path: file_name,
        content: raw,
    }])
}
