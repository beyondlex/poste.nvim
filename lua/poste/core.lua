--- Poste core compatibility stub.
---
--- Since the 2026-09-13 family dissolution, poste.nvim is a pure Rust CLI
--- repository (crates/ + release pipeline). The former shared Lua layer was
--- vendored verbatim into the sibling plugins that used it:
---
---   poste-db.nvim    — state-lite, cli, util, select, layout, dialog,
---                      indicators, installer (+ constants entries)
---   poste-redis.nvim — state-lite, cli, util, select, indicators, installer
---   poste-es.nvim    — state-lite, util, select, dialog, indicators,
---                      installer
---   poste-mail.nvim  — no Lua vendored (already self-hosted compat/)
---
--- The ONLY cross-repo contract left is the `poste` binary CLI/wire
--- surface: see docs/schema.md.
---
--- This stub exists only so old configs calling `require("poste").setup()`
--- keep working (no-op). Once every sibling has shipped a self-contained
--- release and survived a release cycle, delete lua/ and plugin/ entirely.
local M = {}

function M.setup(_opts)
  -- no-op: shared infra is vendored per sibling; the binary installer now
  -- lives in each sibling (e.g. poste-db.install / poste-redis.install_binary)
  -- and downloads from this repo's releases.
end

return M
