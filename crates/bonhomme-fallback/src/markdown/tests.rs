use std::collections::BTreeSet;

use bonhomme_core::{LanguagePlugin, Operation, RenderedFile};
use uuid::Uuid;

use super::{MarkdownHandler, SECTION_KIND};
use crate::test_support::graph_from;

fn rendered(path: &str, content: &str) -> RenderedFile {
    RenderedFile {
        path: path.to_string(),
        content: content.to_string(),
    }
}

const DOC: &str =
    "intro line\n\n# Title\n\nbody of title\n\n## Usage\n\nuse it\n\n## Notes\n\nnote\n";

#[test]
fn import_render_is_byte_identical() {
    let graph = graph_from(
        &MarkdownHandler
            .import(&[rendered("README.md", DOC)])
            .unwrap(),
    );
    assert_eq!(MarkdownHandler.render(&graph)[0].content, DOC);
}

#[test]
fn sections_split_by_heading_with_path_identity() {
    let operations = MarkdownHandler
        .import(&[rendered("README.md", DOC)])
        .unwrap();
    let names: BTreeSet<String> = operations
        .iter()
        .filter_map(|op| match op {
            Operation::CreateSymbol { kind, name, .. } if kind == SECTION_KIND => {
                Some(name.clone())
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        names,
        BTreeSet::from([
            "Title".to_string(),
            "Title > Usage".to_string(),
            "Title > Notes".to_string(),
        ])
    );
}

#[test]
fn editing_one_section_targets_only_that_symbol() {
    let graph = graph_from(
        &MarkdownHandler
            .import(&[rendered("README.md", DOC)])
            .unwrap(),
    );
    let scope: Vec<Uuid> = graph.root_symbols().iter().map(|s| s.id).collect();
    let edited = DOC.replace("use it", "use it well");
    let operations = MarkdownHandler
        .recover_operations(&graph, &scope, &[rendered("README.md", &edited)])
        .unwrap();
    assert!(matches!(
        operations.as_slice(),
        [Operation::UpdateSymbol {
            metadata: Some(_),
            ..
        }]
    ));
}

#[test]
fn repeated_headings_disambiguate_and_import() {
    let doc = "# A\n\n## Examples\n\nx\n\n# B\n\n## Examples\n\ny\n";
    let graph = graph_from(&MarkdownHandler.import(&[rendered("d.md", doc)]).unwrap());
    assert_eq!(MarkdownHandler.render(&graph)[0].content, doc);
    assert_eq!(
        graph
            .symbols
            .values()
            .filter(|s| s.kind == SECTION_KIND)
            .count(),
        4
    );
}

#[test]
fn document_without_headings_round_trips_as_preamble() {
    let doc = "just text\nno headings here\n";
    let graph = graph_from(&MarkdownHandler.import(&[rendered("n.md", doc)]).unwrap());
    assert_eq!(MarkdownHandler.render(&graph)[0].content, doc);
}

#[test]
fn hash_inside_code_fence_is_not_a_heading() {
    let doc = "# Real\n\n```\n# not a heading\n```\n\ntext\n";
    let graph = graph_from(&MarkdownHandler.import(&[rendered("c.md", doc)]).unwrap());
    let sections = graph
        .symbols
        .values()
        .filter(|s| s.kind == SECTION_KIND)
        .count();
    assert_eq!(sections, 1, "only the real heading is a section");
    assert_eq!(MarkdownHandler.render(&graph)[0].content, doc);
}
