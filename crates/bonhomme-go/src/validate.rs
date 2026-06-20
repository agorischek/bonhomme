use anyhow::Result;
use bonhomme_core::RenderedFile;

pub async fn validate_go_files(files: &[RenderedFile]) -> Result<()> {
    crate::toolchain::validate_go_files(files).await
}
