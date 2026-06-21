use anyhow::Result;
use bonhomme_core::RenderedFile;
use pulldown_cmark::Parser;

pub async fn validate_markdown_files(files: &[RenderedFile]) -> Result<()> {
    for file in files {
        // pulldown-cmark is intentionally forgiving; validation here is a parse smoke test that
        // keeps the plugin contract symmetrical with language plugins that do have compilers.
        Parser::new(&file.content).for_each(drop);
    }
    Ok(())
}
