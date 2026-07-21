--- Poste core setup — shared infrastructure for HTTP and SQL plugins.
local state = require("poste.state")
require("poste.util")
require("poste.indicators")
require("poste.constants")

local M = {}

function M.setup(opts)
  opts = opts or {}
  state.config = vim.tbl_deep_extend("force", state.config, opts)

  if vim.g.poste_core_setup_done then
    return
  end
  vim.g.poste_core_setup_done = true
end

function M.status()
  return string.format("[env: %s]", state.current_env)
end

return M