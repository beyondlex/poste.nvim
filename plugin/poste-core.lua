-- Poste core plugin loader — shared infrastructure for HTTP and SQL plugins.
-- This plugin must be installed before poste.nvim or poste-sql.nvim.

local plugin_dir = vim.fn.fnamemodify(debug.getinfo(1, "S").source:sub(2), ":h:h")
local lua_path = plugin_dir .. "/lua/?.lua;" .. plugin_dir .. "/lua/?/init.lua"
if not package.path:find(lua_path, 1, true) then
  package.path = lua_path .. ";" .. package.path
end

local doc_dir = plugin_dir .. "/doc"
if vim.fn.isdirectory(doc_dir) == 1 then
  pcall(vim.cmd.helptags, doc_dir)
end

require("poste.core").setup()