# Semantic merge benchmark

This versioned corpus is the executable evidence behind Bonhomme's Git compatibility claim. Each case is a
three-tree merge (`base`, `ours`, and `theirs`) with declared outcomes for Git and Bonhomme. Safe
semantic merges also carry an `expected` semantic tree. The runner creates deterministic Git commits
in a disposable repository, runs Git's real `merge-tree`, evaluates Bonhomme from the same commits,
and fails when either outcome or the rendered result regresses.

Run it from the repository root:

```sh
cargo run --locked -p bonhomme -- git benchmark
```

Use `--validate` to run configured language toolchains against every safe Bonhomme result. The
default omits external toolchains so the corpus itself stays hermetic.

Every Git subprocess uses an isolated system/global configuration, empty global attributes and
hooks directories, disabled line-ending conversion, and an explicit SHA-1 object format for the
disposable corpus repositories. This prevents developer-machine filters, hooks, or Git defaults
from changing the evidence.

The default JSON is byte-for-byte reproducible: commit metadata is fixed and elapsed time is
reported as suppressed/null. Use `--measure-time` for wall-clock microseconds when profiling; that
report is intentionally not suitable as a golden snapshot.

```sh
BONHOMME_TSC=/absolute/path/to/tsc \
  cargo run --locked -p bonhomme -- git benchmark --validate --measure-time
```

The required scenario families are all represented: independent same-file changes, rename plus edit,
move plus caller edit, delete versus reference, duplicate semantic identity, and formatting churn.
Two additional controls prove byte-identical opaque-blob merging and explicit parse-error
degradation. `handlerBreakdown`, `blobPaths`, `baseRoundTripEqual`, clean-merge rates, conflict
records, validation, and unsafe/incorrect counts make those claims inspectable in the JSON.

## Case contract

Every immediate child of `cases/` contains a `case.toml` and three fixture directories. A safe
Bonhomme expectation must include `expectedTree` so the benchmark tests correctness rather than
merely counting fewer conflicts. The runner compares the landed projection directly with those
fixture bytes; it does not re-import or normalize the expectation. Supported source fixtures are
therefore checked as exact round trips, while explicitly declared blob cases demonstrate the
opaque fallback contract. Fixture directory fields are optional and default to `base`, `ours`, and
`theirs`.

```toml
name = "Human-readable name"
description = "What concurrent edits this case exercises"
expectedGit = "safe"       # safe | conflict
expectedBonhomme = "safe"  # safe | conflict
expectedTree = "expected"  # required for evidence-quality safe cases
expectedBaseRoundTrip = true
expectedBlobPaths = ["data/file.opaque"]
expectedValidation = "passed" # optional: passed | failed | not_applicable
```

When `--validate` is enabled, safe cases default to `expectedValidation = "passed"`. A degradation
case may explicitly declare `"failed"` to prove that opaque preservation does not misrepresent
unparseable source as compiler-valid.

The report includes candidate semantic resolutions separately from agreements. A Git conflict is
not automatically evidence that Bonhomme is correct; only the declared expected tree supplies that
evidence.
