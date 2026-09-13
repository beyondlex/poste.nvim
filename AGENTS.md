# poste.nvim

Pure Rust CLI repository: the `poste` binary (crates/poste-core,
crates/poste-exec, crates/poste-cli) plus the multi-platform release
pipeline. The Neovim plugin family lives in independent, self-contained
sibling repos ([poste-db.nvim](https://github.com/beyondlex/poste-db.nvim),
[poste-redis.nvim](https://github.com/beyondlex/poste-redis.nvim),
[poste-es.nvim](https://github.com/beyondlex/poste-es.nvim),
[poste-mail.nvim](https://github.com/beyondlex/poste-mail.nvim), …) — since
the 2026-09-13 family dissolution they vendor their own Lua and no longer
require this repo on the runtimepath.

## The one contract: the binary surface

The family's only cross-repo interface is the `poste` binary — its
subcommands, flags, stdin/stdout wire formats and the connection-name → URL
resolution rules. All of it is specified in [docs/schema.md](docs/schema.md):

- stdin/stdout NDJSON schemas for `exec-file`, `session`, `redis-exec`,
  `redis-session`, `mq-exec`, `mq-session`, plus the `introspect`, `context`
  and `connection` subcommands;
- `--connection` always takes a Lua-resolved URL (never a name), `--env` and
  `--database` semantics;
- the mirror implementations rule: Lua `resolve_connection_url` (per
  sibling) and Rust `ConnectionConfig::to_url()`
  (`crates/poste-exec/src/sql_connection.rs`) implement the same
  connections.toml + `{{var}}` resolution — neither may drift alone.

Stability: additive flags/fields are MINOR (consumers parse leniently);
removals, renames or event-shape changes are MAJOR and ship in lockstep
with sibling updates. Siblings' health checks probe the binary and report
`poste --version`.

## What's Here

- `crates/` — Rust workspace (poste-core, poste-exec, poste-cli)
- `.github/workflows/` — CI (fmt + clippy `-D warnings` + tests + luacheck)
  and release (multi-platform artifacts + SHA256 checksums)
- `docs/schema.md` — the binary contract (start here for wire formats)
- `lua/poste/` + `plugin/poste-core.lua` — compatibility stub only
  (`require("poste").setup()` no-ops); delete after every sibling has
  shipped a self-contained release and survived a release cycle

## References

| Want | Go to |
|------|-------|
| Binary contract (subcommands, wire formats, resolution rules) | `docs/schema.md` |
| Rust crates | `crates/poste-core/src/`, `crates/poste-exec/src/`, `crates/poste-cli/src/` |
| Build & test | `cargo test` (CI-gated incl. fmt + clippy `-D warnings` + luacheck on the stub) |
| Sibling plugins (UIs, their Lua, their test suites) | the sibling repos above |
