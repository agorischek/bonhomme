# Plan: A Configuration Spine for bonhomme

> **Status:** C1–C3 implemented (`crates/bonhomme/src/config.rs`). `bonhomme.toml`
> is discovered from the repo root; storage URL precedence is flag > env > file >
> default; the default is a project-local embedded Turso DB (`turso:.bonhomme/…`),
> with `postgres://` as an explicit override. Formatter/toolchain/git sections are
> reserved (parsed, not yet wired) and land with their features.

## The reframe

The temptation is to "add a config file." That lumps together four different
needs that live at different layers and demand different remedies. Pulling them
apart is the whole design, because conflating them is how this over-builds.

The four things people will want to control sort into:

| # | Need | Example | Category | When it lands |
|---|------|---------|----------|---------------|
| 1 | **Deployment values** | database URL, toolchain binary paths | config (a value) | **now** |
| 2 | **Behavior tuning** | per-language formatter | config (a value) | with checkout/export |
| 3 | **Operating mode** | write rendered files back for Git | feature flag (a value) | with the coauth-session feature |
| 4 | **Extensibility** | register a custom language plugin | *code*, not config | not a config problem — see below |

The load-bearing distinction: **config is values; extensibility is code.** A
`bonhomme.toml` cleanly serves 1–3. It does *nothing* for 4 — you cannot load a
plugin that isn't compiled in from a TOML file. Registering arbitrary third-party
plugins requires a *loading mechanism* (dynamic libraries, WASM, or a subprocess
protocol à la LSP / how `bonhomme-go` already shells out to a toolchain), each a
separable project with its own ABI-stability and security burden.

And #4 is already handled for today's needs. The `LanguagePlugin` trait *is* the
extensibility seam: a third party adds bonhomme as a dependency and wires their
plugin in at the composition root (`crates/bonhomme/src/plugins.rs`) — exactly
what is happening right now with `bonhomme-rust`. Compile-time plugin composition
is a normal Rust pattern (rustc drivers, tower layers). Runtime registration only
pays off when a consumer genuinely *cannot* compile bonhomme themselves, and there
is no such consumer yet. **Do not build a plugin ABI until there is.**

## Why this is the right v1

Introduce the config **spine** — a small typed loader at the composition root —
and route the knobs we already inject through it. We are not building a config
*system*; we are building one struct, one loader, and a precedence rule.

Three properties make this low-regret:

1. **Optional, with good defaults.** No `bonhomme.toml` ⇒ everything still works
   exactly as today. The file only ever *overrides*. That optionality is the real
   "works out of the box" guarantee — the failure mode to avoid is config-as-
   required, where nothing runs until you author a file.
2. **It lives in the binary, never in `core`/`engine`.** Those crates stay
   agnostic and keep receiving already-resolved values (a database-URL string, a
   constructed plugin registry, later a writeback policy). Config merely *decides*
   the values we already inject through `Storage::connect` and `plugins.rs`. The
   agnostic boundary we built is preserved.
3. **Config is cheapest to add when the configured thing already exists.** That is
   why `database_url` is ready now (Turso makes the URL genuinely vary), formatter
   config waits for a checkout/export path, the Git-writeback flag rides in with
   the coauth-session feature, and plugin loading is not on this roadmap at all.

## Scope

**In scope (this plan):**
- A `Config` type and a loader in the binary crate.
- Discovery of `bonhomme.toml` from the repo root.
- A precedence model: **defaults < `bonhomme.toml` < environment < CLI flag.**
- Routing the one existing knob — `database_url` — through the spine, with the
  default preserved.
- Reserved, documented homes for the next knobs so they slot in without reshaping.

**Explicitly out of scope:**
- A runtime/dynamic plugin-registration system (category 4). Plugins remain
  compile-time via the trait + composition root.
- Formatter execution (waits for a checkout/export boundary — see the formatter
  discussion: configurable formatting must be a checkout/import boundary pass,
  with the internal canonical render left fixed).
- The Git-writeback behavior itself (lands with `coauth-session-plan.md`).

## Design

### Where it lives

A new module `crates/bonhomme/src/config.rs`, owned by the binary (the composition
root). `core` and `engine` never see `Config`; they receive resolved primitives.

```rust
// crates/bonhomme/src/config.rs
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub storage: StorageConfig,
    // reserved — parsed but not yet wired, so the schema is stable as features land:
    pub format: FormatConfig,     // category 2, applied at checkout
    pub git: GitConfig,           // category 3, applied by coauth-session
    pub toolchain: ToolchainConfig, // category 1, binary discovery
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StorageConfig {
    pub database_url: Option<String>,
}
```

### Discovery

`Config::discover(start_dir)` walks upward from the working directory looking for
`bonhomme.toml` (the same ergonomics as `git`, `rustfmt`, `.editorconfig`). Not
found ⇒ `Config::default()`. This also gives us a natural repo-root anchor for the
future `.bonhomme/` working directory in the coauth-session plan.

### Precedence

Resolution happens once, in `cli::run`, lowest to highest:

```
Config::default()
  ← values from bonhomme.toml (if present)
  ← environment (e.g. DATABASE_URL)
  ← CLI flag (e.g. --database-url)
```

`database_url` already supports flag + env via clap; the spine inserts the file
layer *between* the built-in default and env, and makes the resolved value
explicit rather than clap-implicit. The existing `--database-url`/`DATABASE_URL`
behavior is unchanged when no file is present.

### Schema (v1 on disk — every key optional)

```toml
# bonhomme.toml — all keys optional; absent file == today's behavior
[storage]
# postgres://…  |  turso:PATH  |  sqlite:PATH  |  file:PATH  |  :memory:
database_url = "turso:.bonhomme/bonhomme.db"

# --- reserved; parsed (deny_unknown_fields stays happy) but not yet acted on ---
[toolchain]
# go = "go"            # category 1: binary path/discovery for shell-out plugins

[format]
# rust = "canonical"   # category 2: "canonical" | external command; checkout-time only

[git]
# write_back = false   # category 3: lands with the coauth-session feature
```

Wiring only `[storage].database_url` in v1 keeps the change tiny while publishing
the shape the rest will grow into.

## Phased delivery

- **C1 — Spine.** Add `config.rs`: `Config`, `StorageConfig`, `discover`, and a
  `resolve` that applies precedence. No behavior change yet (defaults reproduce
  today's values).
- **C2 — Route storage.** `cli::run` resolves `database_url` through the spine and
  passes it to `Storage::connect`. Add a `--config <path>` override for explicit
  files. Default-when-absent verified.
- **C3 — Reserved homes.** Land the `toolchain`/`format`/`git` structs as
  `#[serde(default)]` no-ops with doc comments pointing at their owning feature,
  so future PRs add a field, not a section.
- **Later (with their features):** toolchain paths feed `bonhomme-go`'s
  `go_binary()`; formatter feeds the checkout/export pass; `git.write_back` feeds
  coauth-session.

## Testing

- `discover` finds the nearest `bonhomme.toml` walking up; returns default when
  none exists.
- Precedence: default < file < env < flag, each layer overriding the last, with a
  test per boundary.
- Absent-file parity: with no config present, the resolved `database_url` equals
  `DEFAULT_DATABASE_URL` (and `--database-url`/`DATABASE_URL` still win).
- `deny_unknown_fields` rejects typos with a clear error (fail-closed beats a
  silently ignored setting).
- A `turso:` URL set only via `bonhomme.toml` drives a full embedded run with no
  flag and no env.

## Risks & open questions

- **Default storage backend.** Should a *project-local* default become
  `turso:.bonhomme/…` so the local workflow needs zero infrastructure, while the
  hosted server keeps `postgres://`? The spine makes this a one-line project
  override either way; flipping the *global* default is a separate call.
- **Config vs. flags drift.** Keep clap as the top precedence layer rather than
  duplicating every flag as a config key — only promote a flag to a config key
  when a value genuinely wants to persist per-project (storage, toolchain), not
  per-invocation.
- **Resisting scope creep.** The doc's job is partly to say *no*: category-4
  plugin registration is not a config feature, and reserved sections must stay
  no-ops until their owning feature exists, so `bonhomme.toml` never advertises a
  knob that does nothing.
```
