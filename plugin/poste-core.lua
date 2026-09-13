-- Poste core plugin loader — compatibility stub only.
-- poste.nvim is a pure Rust CLI repo since the family dissolution; the
-- sibling plugins are self-contained (see docs/schema.md for the binary
-- contract). This loader only keeps require("poste").setup() working until
-- lua/ and plugin/ are deleted after a full sibling release cycle.

local doc_dir = vim.fn.fnamemodify(debug.getinfo(1, "S").source:sub(2), ":h:h") .. "/doc"
if vim.fn.isdirectory(doc_dir) == 1 then
  pcall(vim.cmd.helptags, doc_dir)
end

require("poste.core").setup()
