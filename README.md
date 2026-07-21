# poste.nvim

Shared infrastructure and Rust CLI for [poste-http.nvim](https://github.com/beyondlex/poste-http.nvim) and [poste-sql.nvim](https://github.com/beyondlex/poste-sql.nvim).

**Required by**: poste-http.nvim (HTTP) and poste-sql.nvim (SQL)

## What's included

- `lua/poste/state.lua` — Shared state, config, keymaps, binary discovery
- `lua/poste/constants.lua` — Shared constants
- `lua/poste/select.lua` — Generic Picker UI
- `lua/poste/indicators.lua` — Spinner/✓/✘ indicators
- `lua/poste/util.lua` — Shared utilities
- `lua/poste/cli.lua` — CLI wrapper
- `lua/poste/install.lua` — Binary installer
- `lua/poste/error.lua` — Error handling
- `lua/poste/async/promise.lua` — Promise implementation
- `lua/poste/state/event.lua` — Event bus
- `crates/` — Rust workspace (`poste` CLI binary)

## Installation

```lua
-- lazy.nvim
{
  "beyondlex/poste.nvim",
  lazy = false,
  priority = 1000,
}
```

## Build from source

```bash
cargo build --release
```

## License

MIT