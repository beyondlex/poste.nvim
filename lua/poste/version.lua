--- poste.nvim core version (semver).
---
--- Siblings should capability-check the API they need (e.g.
--- `shared.register_provider ~= nil`, the "legacy_mini_wiring" pattern) rather
--- than compare this value — one poste.nvim copy is loaded per session, so a
--- version number cannot serve two siblings that want different behaviours.
--- This exists for diagnostics and for coordinating contract bumps.
---
--- Semver discipline for lua/poste's public API (AGENTS.md): breaking change
--- -> MAJOR (+ compat shim where feasible), additive -> MINOR, fix -> PATCH.
local M = { major = 1, minor = 0, patch = 0 }

M.string = M.major .. "." .. M.minor .. "." .. M.patch

return M
