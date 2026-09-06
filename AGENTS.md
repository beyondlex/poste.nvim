# poste.nvim

Shared infrastructure and Rust CLI for the Poste ecosystem. Required by [poste-http.nvim](https://github.com/beyondlex/poste-http.nvim) and [poste-db.nvim](https://github.com/beyondlex/poste-db.nvim).

## What's Here

- `lua/poste/` — Shared Lua modules (state.lua, select.lua, indicators.lua, cli.lua, util.lua, text.lua, install.lua, error.lua, async/promise.lua, state/event.lua, constants.lua, core.lua)
- `crates/` — Rust workspace (poste-core, poste-exec, poste-cli)
- `plugin/poste-core.lua` — Plugin loader
- `.opencode/skills/` — Shared skills (arch-review, neovim-performance, refactor)

## Protocol Awareness

This repo is protocol-agnostic. HTTP and SQL are handled by separate repos. Key design rule: shared infra must not import protocol-specific modules (`poste.http.*` or `poste.sql.*`).

## References

| Want | Go to |
|------|-------|
| Rust crates | `crates/poste-core/src/`, `crates/poste-exec/src/`, `crates/poste-cli/src/` |
| Lua shared modules | `lua/poste/` |
| Build & test | `cargo test` (Rust, CI-gated incl. fmt); `tests/run.sh` (Lua, plenary) |
| Agent learnings | `LEARNINGS.md` |
