-- Minimal Neovim configuration for running poste.nvim Lua tests.
-- Used as -u script (actual vimrc replacement). The shared layer has no
-- plugin dependencies, so only the repo itself is added to the runtimepath.

vim.opt.runtimepath:append(".")

package.path = package.path
  .. ";./tests/?.lua"
  .. ";./tests/?/init.lua"
