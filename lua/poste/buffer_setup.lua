local state = require("poste.state")
local M = {}

local function get_nav(buf)
  local ft = vim.api.nvim_buf_get_option(buf, "filetype")
  if ft == "poste_sql" or ft == "poste_sqlite" then
    local ok, mod = pcall(require, "poste-sql.nav")
    if ok then return mod end
  end
  return nil
end

local function get_run_request(buf)
  local ft = vim.api.nvim_buf_get_option(buf, "filetype")
  if ft == "poste_sql" or ft == "poste_sqlite" then
    local ok, mod = pcall(require, "poste-sql.init")
    if ok and mod.run_sql_request then return mod.run_sql_request end
  end
  return nil
end

function M.setup_buffer_keymaps(buf)
  local keymap_opts = { buffer = buf, noremap = true, silent = true }
  local km = state.get_keymap
  local nav = get_nav(buf)
  local run_request = get_run_request(buf)

  if run_request then
    local k = km("sql_source", "run", "<CR>")
    if k then
      vim.keymap.set("n", k, run_request, keymap_opts)
    end
  end

  if nav then
    local k = km("sql_source", "show_ddl", "K")
    if k and nav.show_ddl then vim.keymap.set("n", k, nav.show_ddl, keymap_opts) end
    k = km("sql_source", "format", "<leader>ff")
    if k and nav.format_sql then vim.keymap.set("n", k, nav.format_sql, keymap_opts) end
    k = km("sql_source", "goto_definition", "gd")
    if k and nav.goto_definition then vim.keymap.set("n", k, nav.goto_definition, keymap_opts) end
    k = km("sql_source", "trigger_completion", "<C-Space>")
    if k and nav.trigger_completion then vim.keymap.set("n", k, nav.trigger_completion, keymap_opts) end
  end

  -- Snippet tab-stop navigation (insert mode, only intercept when snippet active)
  local ft = vim.api.nvim_buf_get_option(buf, "filetype")
  if ft == "poste_sql" or ft == "poste_sqlite" then
    local sopts = { buffer = buf, noremap = true, silent = true }
    vim.keymap.set({ "i", "s" }, "<Tab>", function()
      if vim.snippet and vim.snippet.active() then
        vim.snippet.jump(1)
      else
        vim.fn.feedkeys(vim.api.nvim_replace_termcodes("<Tab>", true, true, true), "n")
      end
    end, sopts)
    vim.keymap.set({ "i", "s" }, "<S-Tab>", function()
      if vim.snippet and vim.snippet.active() then
        vim.snippet.jump(-1)
      else
        vim.fn.feedkeys(vim.api.nvim_replace_termcodes("<S-Tab>", true, true, true), "n")
      end
    end, sopts)

    end

  local indicator_ns = vim.api.nvim_create_namespace("poste_indicator")
  local group = vim.api.nvim_create_augroup("PosteClearIndicators_" .. buf, { clear = true })
  vim.api.nvim_create_autocmd("TextChanged", {
    group = group, buffer = buf,
    callback = function()
      vim.api.nvim_buf_clear_namespace(buf, indicator_ns, 0, -1)
    end,
  })
end

return M