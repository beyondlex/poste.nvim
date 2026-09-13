# Poste

[![CI](https://github.com/beyondlex/poste.nvim/actions/workflows/ci.yml/badge.svg)](https://github.com/beyondlex/poste.nvim/actions/workflows/ci.yml)

**Send requests from files. Keyboard-first. Multi-protocol.**

`poste.nvim` ships the `poste` Rust CLI: a fast, standalone executor for
requests described in plain text — SQL files with streaming progress,
persistent SQL/Redis/AMQP sessions, database introspection and SQL text
analysis. Inspired by the JetBrains HTTP Client, with NDJSON streaming output
that thin Neovim frontends render as spreadsheet-like datasets.

## The sibling plugins (independent projects)

The Neovim UIs are separate, self-contained repositories — install the ones
you need; they evolve and release independently:

| Plugin | Protocol |
|--------|----------|
| [poste-http.nvim](https://github.com/beyondlex/poste-http.nvim) | HTTP / REST (`.http` / `.rest` files) |
| [poste-db.nvim](https://github.com/beyondlex/poste-db.nvim) | SQL — PostgreSQL, MySQL/MariaDB, SQLite, MSSQL, ClickHouse; dataset browser, editing, schema introspection |
| [poste-redis.nvim](https://github.com/beyondlex/poste-redis.nvim) | Redis — key browser, command runner |
| [poste-mq.nvim](https://github.com/beyondlex/poste-mq.nvim) | AMQP — queue inspection, tail |
| [poste-es.nvim](https://github.com/beyondlex/poste-es.nvim) | Elasticsearch |
| [poste-mail.nvim](https://github.com/beyondlex/poste-mail.nvim) | Mail |
| [poste-ai.nvim](https://github.com/beyondlex/poste-ai.nvim) | Optional AI chat with per-connection context injection |

The plugins shell out to the `poste` binary (installed automatically from
[this repo's releases](#installation), or point `vim.g.poste_binary` at your
own build). The binary's CLI surface and wire formats are the family's one
cross-repo contract — documented in [docs/schema.md](docs/schema.md).

## Installation

Build from source, or install a prebuilt binary:

```bash
cargo install --path crates/poste-cli   # installs `poste` onto your PATH
```

`v*` tags cut a release; [release.yml](.github/workflows/release.yml) builds
binaries for x86_64-linux, aarch64-linux, aarch64-macos, x86_64-macos and
x86_64-windows with SHA256 checksums. The sibling plugins' vendored
installers download these same assets on first setup.

## The `poste` CLI

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

Examples (the plugins drive these automatically; flags via `--help`):

```bash
poste exec-file queries.sql --connection postgres://user@host:5432/app --env dev
poste introspect --connection-url postgres://user@host:5432/app --type tables --schema public
poste connection list --env dev
```

## Configuration

Requests reference environments and connections through `{{var}}`
substitution, resolved from JetBrains-style `env.json`, `.env` files and the
OS environment. Connections are defined per project in `connections.toml`
(dialect, host, port, user, password, database — secrets via the same env
vars). Each sibling plugin's README documents its exact format and options;
the resolution rules and every wire format are specified in
[docs/schema.md](docs/schema.md).

## Development

```bash
cargo build --release              # Rust CLI
cargo test --workspace             # Rust tests
cargo clippy --workspace --all-targets -- -D warnings   # zero-warning gate
cargo fmt --all --check            # formatting gate
```

Layout: `crates/poste-core` (request/SQL parsing, env management),
`crates/poste-exec` (protocol executors), `crates/poste-cli` (the binary).
`lua/poste/` is a compatibility stub (`require("poste").setup()` no-ops)
kept until every sibling has shipped a self-contained release; the Lua
plugin family now lives in the sibling repos and is tested there.

## License

[MIT](LICENSE)
