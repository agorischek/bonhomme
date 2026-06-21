use anyhow::Result;
use bonhomme_core::RenderedFile;

pub async fn validate_go_files(files: &[RenderedFile]) -> Result<()> {
    crate::toolchain::validate_go_files(files).await
}

pub async fn validate_go_files_with_workspace(
    all_files: &[RenderedFile],
    go_files: &[RenderedFile],
) -> Result<()> {
    crate::toolchain::validate_go_files_with_workspace(all_files, go_files).await
}
