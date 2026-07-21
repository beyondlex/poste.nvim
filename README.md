# Poste

**Send requests from files. Keyboard-first. Multi-protocol.**

A Neovim plugin and Rust CLI for executing HTTP, Redis, SQL (PostgreSQL / MySQL / SQLite) requests from plain text files. Inspired by JetBrains HTTP Client, with focus on keyboard-driven workflows and dataset manipulation.

## Features

- **File-based requests** — Define requests in `.http`/`.rest`, `.sql`, `.redis` files
- **Environment variables** — JetBrains-style `env.json` with `{{var}}` substitution
- **Named connections** — `connections.json` for database credentials; supports env var references
- **Keyboard-first** — Execute at cursor, navigate results with Vim keys, never leave home row
- **Multi-protocol** — HTTP, Redis, PostgreSQL, MySQL, SQLite

## Repositories

Poste is split into three repositories:

| Repo | Description |
|------|-------------|
| [poste.nvim](https://github.com/beyondlex/poste.nvim) | Shared infrastructure, Rust CLI, build system *(this repo)* |
| [poste-http.nvim](https://github.com/beyondlex/poste-http.nvim) | HTTP + Redis protocol execution, Neovim UI |
| [poste-sql.nvim](https://github.com/beyondlex/poste-sql.nvim) | SQL execution, dataset browser, schema introspection |

### Architecture

```
poste.nvim/                    ← shared infra + Rust CLI
├── crates/
│   ├── poste-core/            # Request parsing, SQL parsing, env management
│   ├── poste-exec/            # Protocol execution, SQL connection/dialect
│   └── poste-cli/             # CLI binary (run / connection / introspect)
├── lua/poste/                 # Shared Lua infra (state, select, constants, cli, etc.)
├── plugin/poste-core.lua
└── tests/

poste-http.nvim/               ← HTTP + Redis
├── lua/poste/http/            # HTTP protocol modules
├── plugin/poste.lua
└── tests/

poste-sql.nvim/                ← SQL (optional)
├── lua/poste/sql/             # SQL protocol modules
├── plugin/poste-sql.lua
└── tests/sql/
```

## Installation

### With HTTP (recommended)

```lua
-- lazy.nvim
{
  "beyondlex/poste-http.nvim",
  dependencies = {
    "beyondlex/poste.nvim",
    "saghen/blink.cmp",
    "stevearc/dressing.nvim",
    "beyondlex/finder",
  },
  config = function()
    require("poste").setup()
  end,
}
```

### With SQL (add to dependencies)

```lua
{
  "beyondlex/poste-sql.nvim",
  dependencies = {
    "beyondlex/poste.nvim",
  },
  config = function()
    require("poste.sql.init").setup()
  end,
}
```

### Rust CLI (optional)

```bash
cargo build --release
```

The CLI enables standalone execution (`poste run`) and context-aware features (completion, introspection). The plugin discovers it automatically in PATH or `stdpath("data")/poste/bin/poste`.

## CLI

```bash
# Execute a request by line number
poste run requests/api.http --line 4 --env dev

# Introspect database schema
poste introspect --connection pg-dev --env dev

# List available connections
poste connection list --env dev

# Format HTTP file
poste fmt requests/api.http

# Import OpenAPI / Postman collection
poste import openapi spec.yaml
```

## Development Status

**Progress: 34/38 steps completed** (~90%)

| Phase | Description | Status |
|-------|-------------|--------|
| **1A** | Rust infrastructure | ✅ Complete |
| **1B** | Lua dataset panel | ✅ Complete |
| **1C** | MySQL/SQLite executors | ✅ Complete |
| **2** | Connection & context management | ✅ Complete |
| **3** | DB structure browser | ✅ Complete |
| **4** | Table operations + DDL + completion | ✅ Complete |
| **5** | Import/export + pagination | ✅ Complete |
| **6** | Advanced features (editor, transactions) | ✅ Complete |

**Tests:** 300+ passing (230 Rust + 70 Lua)

## License

MIT