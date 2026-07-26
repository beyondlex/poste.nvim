-- Poste core plugin loader — shared infrastructure for HTTP and SQL plugins.
-- This plugin must be installed before poste-http.nvim or poste-sql.nvim.

local doc_dir = vim.fn.fnamemodify(debug.getinfo(1, "S").source:sub(2), ":h:h") .. "/doc"
if vim.fn.isdirectory(doc_dir) == 1 then
  pcall(vim.cmd.helptags, doc_dir)
end

require("poste.core").setup()