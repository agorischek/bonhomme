use std::collections::BTreeSet;

use bonhomme_core::{
    LanguagePlugin, Operation, OperationRecord, RenderedFile, SemanticGraph, materialize,
    metadata_string,
};
use uuid::Uuid;

use crate::{
    MarkdownPlugin,
    model::{
        CODE_BLOCK_KIND, FRONTMATTER_KIND, IMAGE_KIND, LINK_KIND, LINKS_TO_KIND, SECTION_KIND,
    },
};

fn rendered(path: &str, content: &str) -> RenderedFile {
    RenderedFile {
        path: path.to_string(),
        content: content.to_string(),
    }
}

fn graph_from(operations: &[Operation]) -> SemanticGraph {
    let records = operations
        .iter()
        .enumerate()
        .map(|(index, operation)| OperationRecord {
            id: Uuid::new_v4(),
            repository_id: Uuid::nil(),
            branch_id: Uuid::nil(),
            changeset_id: Uuid::nil(),
            position: index as i64 + 1,
            operation: operation.clone(),
            created_at: chrono::DateTime::<chrono::Utc>::UNIX_EPOCH,
        })
        .collect::<Vec<_>>();
    materialize(&records).expect("operations materialize")
}

const DOC: &str =
    "intro line\n\n# Title\n\nbody of title\n\n## Usage\n\nuse it\n\n## Notes\n\nnote\n";

#[test]
fn import_render_is_byte_identical() {
    let graph = graph_from(
        &MarkdownPlugin
            .import(&[rendered("README.md", DOC)])
            .unwrap(),
    );

    assert_eq!(MarkdownPlugin.render(&graph)[0].content, DOC);
}

#[test]
fn sections_are_nested_by_heading() {
    let graph = graph_from(
        &MarkdownPlugin
            .import(&[rendered("README.md", DOC)])
            .unwrap(),
    );
    let file = graph
        .root_symbols()
        .into_iter()
        .find(|symbol| symbol.kind == "file")
        .unwrap();
    let title = graph
        .children_of(file.id)
        .into_iter()
        .find(|symbol| symbol.kind == SECTION_KIND && symbol.name == "Title")
        .unwrap();
    let child_names = graph
        .children_of(title.id)
        .into_iter()
        .filter(|symbol| symbol.kind == SECTION_KIND)
        .map(|symbol| symbol.name.clone())
        .collect::<BTreeSet<_>>();

    assert_eq!(
        child_names,
        BTreeSet::from(["Notes".to_string(), "Usage".to_string()])
    );
}

#[test]
fn frontmatter_code_blocks_links_and_images_are_symbols() {
    let doc = "---\ntitle: Demo\n---\n\n# Title\n\n![Logo](logo.png)\n\n```rust\nfn main() {}\n```\n\n[Next](next.md)\n";
    let graph = graph_from(
        &MarkdownPlugin
            .import(&[rendered("README.md", doc)])
            .unwrap(),
    );
    let kinds = graph
        .symbols
        .values()
        .map(|symbol| symbol.kind.clone())
        .collect::<BTreeSet<_>>();

    assert!(kinds.contains(FRONTMATTER_KIND));
    assert!(kinds.contains(CODE_BLOCK_KIND));
    assert!(kinds.contains(IMAGE_KIND));
    assert!(kinds.contains(LINK_KIND));
    assert_eq!(MarkdownPlugin.render(&graph)[0].content, doc);
}

#[test]
fn heading_links_create_references() {
    let doc = "# Intro\n\nSee [Usage](#usage).\n\n## Usage\n\nUse it.\n";
    let graph = graph_from(
        &MarkdownPlugin
            .import(&[rendered("README.md", doc)])
            .unwrap(),
    );
    let link = graph
        .symbols
        .values()
        .find(|symbol| symbol.kind == LINK_KIND)
        .unwrap();
    let usage = graph
        .symbols
        .values()
        .find(|symbol| {
            symbol.kind == SECTION_KIND
                && metadata_string(&symbol.metadata, "anchor").as_deref() == Some("usage")
        })
        .unwrap();

    assert!(graph.references.values().any(|reference| {
        reference.kind == LINKS_TO_KIND
            && reference.from_symbol_id == link.id
            && reference.to_symbol_id == usage.id
    }));
}

#[test]
fn cross_document_heading_links_create_references() {
    let files = [
        rendered(
            "README.md",
            "# Home\n\nSee [Install](docs/guide.md#install).\n",
        ),
        rendered("docs/guide.md", "# Install\n\nRun it.\n"),
    ];
    let graph = graph_from(&MarkdownPlugin.import(&files).unwrap());
    let install = graph
        .symbols
        .values()
        .find(|symbol| {
            symbol.kind == SECTION_KIND
                && metadata_string(&symbol.metadata, "identityPath").as_deref() == Some("Install")
        })
        .unwrap();

    assert!(
        graph.references.values().any(
            |reference| reference.to_symbol_id == install.id && reference.kind == LINKS_TO_KIND
        )
    );
}

#[test]
fn editing_one_section_targets_only_that_section() {
    let graph = graph_from(
        &MarkdownPlugin
            .import(&[rendered("README.md", DOC)])
            .unwrap(),
    );
    let scope: Vec<Uuid> = graph
        .root_symbols()
        .iter()
        .map(|symbol| symbol.id)
        .collect();
    let edited = DOC.replace("use it", "use it well");
    let operations = MarkdownPlugin
        .recover_operations(&graph, &scope, &[rendered("README.md", &edited)])
        .unwrap();

    assert_eq!(operations.len(), 1, "{operations:#?}");
    let Operation::UpdateSymbol {
        symbol_id,
        body: Some(body),
        ..
    } = &operations[0]
    else {
        panic!("expected one section body update, got {operations:#?}");
    };
    let symbol = graph.symbols.get(symbol_id).unwrap();
    assert_eq!(symbol.kind, SECTION_KIND);
    assert_eq!(symbol.name, "Usage");
    assert!(body.contains("use it well"));
}
