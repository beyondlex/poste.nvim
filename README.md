# Poste

[![CI](https://github.com/beyondlex/poste.nvim/actions/workflows/ci.yml/badge.svg)](https://github.com/beyondlex/poste.nvim/actions/workflows/ci.yml)

**Send requests from files. Keyboard-first. Multi-protocol.**

A Neovim plugin and Rust CLI for executing requests from plain text files —
`.http`/`.rest` for HTTP, `.sql` for databases, and protocol plugins for
Redis, AMQP, Elasticsearch and mail. Inspired by the JetBrains HTTP Client,
with a focus on keyboard-driven workflows and a spreadsheet-like dataset UI.

<!--
Hero screenshot: dataset browser with winbar context + SQL statusline.
Drop the image at .github/assets/dataset.png and uncomment.
![Poste dataset browser](.github/assets/dataset.png)
-->

## How it fits together

`poste.nvim` is the shared foundation of the Poste plugin family: it ships the
`poste` Rust binary (parsing, protocol executors), the shared Lua
infrastructure (state, connection lookup, pickers, layout/dialog rendering,
sign-column indicators, one shared statusline context layer) and the binary
installer. Protocol plugins build on it:

| Plugin | Protocol |
|--------|----------|
| [poste-http.nvim](https://github.com/beyondlex/poste-http.nvim) | HTTP / REST (`.http` / `.rest` files) |
| [poste-db.nvim](https://github.com/beyondlex/poste-db.nvim) | SQL — PostgreSQL, MySQL/MariaDB, SQLite, MSSQL, ClickHouse; dataset browser, editing, schema introspection |
| [poste-redis.nvim](https://github.com/beyondlex/poste-redis.nvim) | Redis — key browser, command runner |
| [poste-mq.nvim](https://github.com/beyondlex/poste-mq.nvim) | AMQP — queue inspection, tail |
| [poste-es.nvim](https://github.com/beyondlex/poste-es.nvim) | Elasticsearch |
| [poste-mail.nvim](https://github.com/beyondlex/poste-mail.nvim) | Mail |
| [poste-ai.nvim](https://github.com/beyondlex/poste-ai.nvim) | Optional AI chat with per-connection context injection |

Install the ones you need; they coexist in one session and share the same
statusline context, connection store and binary.

## Requirements

- Neovim 0.10+
- `curl` and `tar` — only for the automatic binary install (below)
- Optional: [snacks.nvim](https://github.com/folke/snacks.nvim) for the picker
  UI (a built-in float is used when absent); per-protocol extras (e.g.
  `blink.cmp` for poste-db completion) are documented in each sibling README

## Installation

Add a protocol plugin to your config; it pulls `poste.nvim` in as a
dependency. No config function needed — loading `poste.nvim` runs
`require("poste.core").setup()` automatically:

```lua
-- lazy.nvim — SQL
{ "beyondlex/poste-db.nvim", dependencies = { "beyondlex/poste.nvim" } }

-- lazy.nvim — HTTP
{ "beyondlex/poste-http.nvim", dependencies = { "beyondlex/poste.nvim" } }
```

On first load, `poste.nvim` automatically installs the `poste` binary: it
downloads the prebuilt platform archive from GitHub Releases, verifies its
SHA256 checksum and unpacks it to `stdpath("data")/poste/bin/poste`.

Alternative binary sources:

- Point at your own build: `vim.g.poste_binary = "/path/to/poste"` — handy to
  test unreleased Rust work from a checkout or worktree (`target/{debug,release}/poste`
  next to the cwd or the poste.nvim runtime dir is also picked up).
- Build from source: `cargo install --path crates/poste-cli` (installs `poste`
  onto your `PATH`).

The resolution order is `vim.g.poste_binary` → config →
`stdpath("data")/poste/bin` / PATH-adjacent build outputs → `PATH`.

## The `poste` CLI

Everything the UIs do is available standalone — the plugins are thin frontends
that shell out to the same binary:

```
poste              Execute requests from files
  connection       Manage SQL connections
  introspect       Introspect database structure (databases, schemas, tables, columns, indexes)
  context          SQL context detection (for completion / indicator placement)
  exec-file        Execute a SQL file with streaming progress
  session          Persistent SQL session (connection alive across requests)
  redis-exec       Execute redis commands (stdin JSON: {"connection", "commands"})
  redis-session    Persistent redis session
  mq-exec          Execute AMQP operations (stdin JSON: {"connection", "operations"})
  mq-session       Persistent AMQP session with push consumers
```

Examples (the plugin drives these automatically; flags via `--help`):

```bash
poste exec-file queries.sql --connection postgres://user@host:5432/app --env dev
poste introspect --connection pg-dev --env dev
poste connection list --env dev
```

## Configuration

Requests reference environments and connections through `{{var}}`
substitution, resolved from JetBrains-style `env.json`, `.env` files and the
OS environment. Connections are defined per project in `connections.toml`
(dialect, host, port, user, password, database — secrets via the same env
vars). Each protocol plugin's README documents its exact format and options;
the shared layer only resolves names to URLs and never sees your secrets in
logs.

<!--
More screenshots worth adding (one line each, under the matching sibling
README instead if you prefer):
![db browser](.github/assets/db-browser.png)
![search highlight](.github/assets/search.png)
![connection picker](.github/assets/picker.png)
-->

## Development

```bash
cargo build --release              # Rust CLI
cargo test --workspace             # Rust tests
cargo clippy --workspace --all-targets -- -D warnings   # zero-warning gate
cargo fmt --all --check            # formatting gate
./tests/run.sh                     # Lua tests (plenary; needs nvim + plenary.nvim)
```

Layout: `crates/poste-core` (request/SQL parsing, env management),
`crates/poste-exec` (protocol executors), `crates/poste-cli` (the binary),
`lua/poste/` (shared Lua: `state`, `cli`, `install`, `select`, `layout`,
`dialog`, `indicators`, `statusline`, `util`, `error`, `version`).

Design rule: the shared layer is protocol-agnostic — it never imports
`poste.http.*`, `poste-db.*` or any sibling module. Sibling contracts (shared
state ownership, the statusline provider API, semver discipline) are
documented in [AGENTS.md](AGENTS.md).

Tag `v*` to cut a release; [release.yml](.github/workflows/release.yml)
builds binaries for x86_64-linux, aarch64-linux, aarch64-macos, x86_64-macos
and x86_64-windows with SHA256 checksums — the same assets the installer
downloads.

## License

[MIT](LICENSE)
