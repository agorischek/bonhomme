# Plan: TypeScript TSDoc Preservation

**Status:** proposed.
**Companion reading:** [structural-identity-plan.md](structural-identity-plan.md)
(clean slices and graph-anchored recovery), [core-premise.md](core-premise.md)
(files are projections).

## Goal

Preserve TypeScript documentation comments as first-class symbol metadata through
import, render, slice apply, and merge. Today the TypeScript plugin parses the
code structure but does not understand TSDoc/JSDoc comments. Some comments may
survive incidentally as raw source text, but they are not attached to the symbol
they document, and member-level docs can be dropped when the file is re-rendered.

Success looks like:

- Leading `/** ... */` comments attached to classes, functions, methods, getters,
  setters, and properties round-trip byte-for-byte except for intentional
  indentation changes.
- Rendered files keep documentation immediately above the symbol they document.
- Slice edits can add, update, or delete docs and produce normal `UpdateSymbol`
  operations.
- TSDoc text is available in graph metadata for future docs/search/review UI.
- Non-documentation comments that are not attached to a supported symbol keep the
  current preamble/body behavior.

## Why

For TypeScript, TSDoc is part of the public API surface. It feeds editor hover
text, generated docs, typed package consumption, and code review context. Losing
or relocating it makes bonhomme's projection less faithful even when the code
still compiles.

This is not just formatting fidelity. A method with a changed implementation but
stale `@returns` text is a semantic review issue. A public function with missing
`@deprecated` or `@throws` documentation can change how downstream users consume
the API. The TypeScript plugin should therefore treat documentation comments as
symbol-owned data, not as anonymous trivia.

## Current State

- `crates/bonhomme-ts/src/import.rs` imports files, classes, methods,
  properties, top-level functions, and call references.
- `crates/bonhomme-ts/src/oxc_parse.rs` stores declaration/signature/body text
  by slicing AST spans from source.
- `outside_ranges` stores text outside imported top-level ranges as file
  `preamble` metadata.
- `render.rs` prints file preamble first, then renders symbol declarations,
  signatures, and bodies from graph metadata.
- `strip_symbol_comments` removes only old bonhomme identity comments; it does
  not model normal comments.

This means the plugin can preserve comments only accidentally. In particular:

- A top-level doc comment may land in file preamble instead of staying attached
  to the following class or function.
- A class-member doc comment before a method or property is not explicitly
  stored and can disappear during render.
- TSDoc tags such as `@param`, `@returns`, `@deprecated`, and `@example` are not
  parsed or indexed.

## Design

Represent documentation comments as optional symbol metadata:

```json
{
  "doc": "/**\n * Formats an order id.\n * @param id - Raw order id.\n */"
}
```

Keep the original block text, including `/**` and `*/`, because this is the
lowest-risk representation for round-tripping. Later enhancements can parse tags
into structured fields, but v1 should preserve the source faithfully before it
tries to interpret it.

### Ownership Rules

A doc comment belongs to the nearest supported symbol when all of these are true:

1. The comment is a block comment whose opening is `/**`.
2. Only whitespace and ordinary line comments appear between the comment and the
   symbol declaration.
3. The symbol is one of: class, top-level function, method, getter, setter, or
   property.

Do not attach:

- `/* ... */` implementation notes that are not TSDoc-style docs.
- Trailing comments after declarations.
- Comments separated from a declaration by executable code or another
  declaration.
- File/license headers unless they are immediately attached to an exported
  symbol by the ownership rules.

When a comment is attached to a symbol, remove its range from file preamble so it
does not render twice. Unattached comments remain in preamble or body text as
they do today.

### Parser Strategy

Use OXC's comment data if it exposes the needed ranges in the current dependency.
If not, add a small lexical scanner for block comments:

```text
source text -> doc comment ranges -> AST symbol start offsets -> attach nearest
```

The scanner only needs to identify `/** ... */` ranges outside strings/templates.
Prefer OXC comment metadata if available because it avoids maintaining another
source scanner. The fallback scanner is acceptable if it is tightly tested and
does not try to become a TypeScript lexer.

The import path should create a `DocComments` helper:

```rust
struct DocComment {
    start: usize,
    end: usize,
    text: String,
}

struct DocComments {
    comments: Vec<DocComment>,
}

impl DocComments {
    fn take_leading_for(&mut self, symbol_start: usize, source: &str) -> Option<String>;
    fn claimed_ranges(&self) -> Vec<(usize, usize)>;
}
```

`take_leading_for` attaches at most one leading doc block. Multiple adjacent doc
blocks should stay together only if TypeScript/TSDoc tools commonly treat them as
one doc block; otherwise preserve the last doc block as attached and leave the
earlier block in preamble. This should be decided with a small fixture before
implementation.

### Metadata Shape

Store docs in existing symbol metadata:

| Symbol kind | Existing metadata | Add |
|---|---|---|
| `class` | `declaration`, `exported` | `doc` |
| `function` | `declaration`, `exported` | `doc` |
| `method` / accessors | `signature`, `methodKind`, `static` | `doc` |
| `property` | `declaration` | `doc` |

Do not put documentation into `declaration` or `signature`. Keeping docs
separate makes diff/recovery cleaner and lets future UI show documentation
without parsing TypeScript text.

### Rendering

Before rendering a documented symbol:

1. Write the doc comment at the symbol's indentation.
2. Re-indent internal lines relative to the symbol indentation.
3. Render the existing declaration/signature exactly as today.

Example:

```ts
export class OrderService {
  /**
   * Returns a display label for an order.
   * @param id - Order id.
   */
  displayName(id: OrderId): string {
    return formatOrder(id);
  }
}
```

The renderer should not parse or wrap doc text. It should preserve the imported
comment text and only normalize leading indentation so nested methods render
cleanly.

### Recovery And Diff

`parse.rs` should include `doc: Option<String>` on parsed classes, functions, and
methods. If properties become part of structural recovery, include property docs
there as well.

Graph-anchored recovery should compare doc metadata alongside name, signature,
and body:

- Changed docs on an existing symbol -> `UpdateSymbol { metadata: Some(...) }`.
- Added docs -> `UpdateSymbol` adding `doc`.
- Deleted docs -> `UpdateSymbol` removing `doc` or setting it to `null`,
  depending on the core metadata merge semantics.

Before implementing delete semantics, confirm how `UpdateSymbol.metadata` handles
removing keys. If it only merges keys, add a core-supported metadata deletion
path or represent absence with an explicit `doc: null` convention and teach
rendering to treat null as absent.

The legacy two-file diff path should get the same behavior so older comment-based
slice fixtures do not regress.

## Phases

### P0 - Characterize Current Behavior

Add failing tests that prove the gap:

- Top-level function TSDoc currently moves to preamble or is not attached.
- Class TSDoc should render directly above the class.
- Method TSDoc should render inside the class above the method.
- Property TSDoc should render inside the class above the property.
- Editing only a doc comment in a stored slice should produce an update.

These tests should be written against the intended behavior and initially fail.

### P1 - Import And Render Preservation

Implement doc comment extraction and metadata storage for import/render only.

Acceptance:

- `import_typescript_files` stores `doc` metadata on supported symbols.
- `render_files` emits docs at the correct indentation.
- Attached docs are excluded from file preamble.
- Existing non-doc comment behavior stays unchanged.
- Existing TypeScript tests continue to pass.

### P2 - Parse, Diff, And Stored-Slice Recovery

Thread `doc` through parsed models and operation recovery.

Acceptance:

- Legacy `diff_slice(original, modified)` detects doc-only edits.
- Stored-slice `recover_operations` detects doc-only edits.
- Add/update/delete doc edits are deterministic and scoped to rendered symbols.
- Call-reference recovery is unaffected.

### P3 - Structured TSDoc Tags (Optional)

After raw preservation is stable, optionally parse docs into structured metadata:

```json
{
  "doc": "/** ... */",
  "tsdoc": {
    "summary": "Formats an order id.",
    "tags": [
      { "name": "param", "text": "id - Raw order id." }
    ]
  }
}
```

This is intentionally out of v1. Raw preservation is the product-critical piece;
tag parsing is useful for search and review UI but should not block fidelity.

## Tests

Add focused tests in `crates/bonhomme-ts/src/tests.rs` and
`crates/bonhomme-ts/src/tests/recover.rs`:

- `import_preserves_top_level_function_tsdoc`
- `import_preserves_class_tsdoc`
- `import_preserves_method_tsdoc`
- `import_preserves_property_tsdoc`
- `render_reindents_nested_tsdoc`
- `diff_detects_function_doc_update`
- `recover_detects_method_doc_update`
- `recover_detects_doc_delete`
- `unattached_comments_remain_in_preamble`
- `ordinary_block_comments_do_not_become_docs`

Also add at least one compiler-backed round-trip test so the rendered output is
validated by the configured TypeScript compiler.

## Open Questions

- Does the current OXC parser expose comment ranges with enough fidelity, or do
  we need a local doc-comment scanner?
- Should multiple adjacent `/** ... */` blocks be joined or should only the last
  block attach to the symbol?
- What is the cleanest core representation for deleting a metadata key during an
  `UpdateSymbol`?
- Should file-level TSDoc be modeled later as file symbol documentation, or
  should license/header comments remain purely textual preamble?

## Non-Goals

- Full TSDoc semantic validation.
- Reformatting or wrapping documentation text.
- Preserving every arbitrary trivia edge case in TypeScript source.
- Modeling inline comments inside function bodies beyond the existing body-text
  preservation behavior.
- Changing symbol identity recovery or handler routing.

## Completion Checklist

- Docs preserve across import -> graph -> render for top-level and class-member
  symbols.
- Docs preserve across slice create -> edit -> apply -> render.
- Doc-only edits produce reviewable semantic operations.
- Existing TypeScript round-trip and validation tests pass.
- `docs/spec-coverage.md` is updated to mention TypeScript doc-comment
  preservation and any remaining known limits.
