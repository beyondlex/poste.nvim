# poste.nvim

Shared infrastructure and Rust CLI for the Poste plugin family. Required by [poste-http.nvim](https://github.com/beyondlex/poste-http.nvim), [poste-db.nvim](https://github.com/beyondlex/poste-db.nvim), [poste-redis.nvim](https://github.com/beyondlex/poste-redis.nvim), [poste-es.nvim](https://github.com/beyondlex/poste-es.nvim), and others.

## What's Here

- `lua/poste/` — Shared Lua modules: `state.lua` (config singleton, binary resolution, highlight overrides), `cli.lua` (poste binary wrapper), `select.lua`, `layout.lua`, `dialog.lua`, `indicators.lua`, `statusline.lua` (shared context compose layer), `util.lua`, `error.lua`, `install.lua`, `constants.lua`, `version.lua`, `core.lua`
- `crates/` — Rust workspace (poste-core, poste-exec, poste-cli)
- `plugin/poste-core.lua` — Plugin loader
- `.opencode/skills/` — Shared skills (arch-review, neovim-performance, refactor)

## Protocol Awareness

This repo is protocol-agnostic. HTTP, SQL, Redis, ES, MQ are handled by separate repos. Key design rule: shared infra must not import protocol-specific modules (`poste.http.*`, `poste.sql.*`, `poste-redis.*`, ...).

## Shared-Surface Contracts

Siblings co-load in one Neovim session, so everything in `lua/poste/` is a
**global, single-instance surface**. Two contracts keep them from breaking
each other:

### state.lua ownership rules

The `poste.state` singleton is shared mutable state for the whole session:

- ALLOWED for siblings: read `config`, `current_env` (read-mostly; nothing in
  the family currently writes it), `get_keymap`/`format_keymap`,
  `find_poste_binary`, `apply_highlight_overrides`, `log`.
- FORBIDDEN: dynamically attaching mutable fields (e.g. `poste_state.connection
  = name`). Each sibling keeps per-plugin state in its own `lua/<plugin>/state.lua`;
  a field two siblings both write becomes last-writer-wins and silently changes
  the other's behaviour (this bit redis/es via `poste_state.connection`).
  If cross-plugin sharing is ever genuinely needed, add an explicit accessor
  here with namespacing and document it — never reach across ad hoc.

### statusline.lua provider contract

`poste.statusline` is the ONE owner of the mini.statusline context hooks —
siblings register providers, never wire mini.statusline themselves. The full
contract (spec shape, scope semantics, hl namespacing `Poste<Plugin>Ctx*`,
resolve-cheap rule, error isolation, re-register semantics) lives in the
`lua/poste/statusline.lua` header comment, and `tests/poste/statusline_spec.lua`
pins it with two coexisting providers (db-like + redis-like). Changing the
compose layer means updating that spec first.

### API stability

No version pinning is possible per sibling (one rtp copy, one loaded module),
so stability comes from the contracts above plus semver on `lua/poste`'s
public API (`lua/poste/version.lua`): breaking change -> MAJOR with a compat
shim where feasible (see `poste-db/lua/poste-db/compat.lua` for the pattern);
additive -> MINOR; fix -> PATCH. Siblings should capability-check
(`shared.register_provider ~= nil`) rather than compare versions.

## References

| Want | Go to |
|------|-------|
| Rust crates | `crates/poste-core/src/`, `crates/poste-exec/src/`, `crates/poste-cli/src/` |
| Lua shared modules | `lua/poste/` |
| Build & test | `cargo test` (Rust, CI-gated incl. fmt); `tests/run.sh` (Lua, plenary) |
| Agent learnings | `LEARNINGS.md` |
