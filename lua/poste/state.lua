local M = {}

M.config = {
  poste_binary = vim.fn.stdpath("data") .. "/poste/bin/poste",
  default_env = "dev",
  split_direction = "vertical",
  split_size = 80,
  log_file = vim.fn.stdpath("cache") .. "/poste.log",
  response_cache_dir = vim.fn.stdpath("cache") .. "/poste_res",
  max_body_bytes = 100 * 1024,
  max_body_lines = 500,
  body_preview_lines = 20,
  sql_formatters = { "sqlfluff", "sqlfmt", "sql-formatter", "pg_format" },
  keymaps = {
    sql_source = {
      run = "<CR>",
      show_ddl = "K",
      format = "<leader>ff",
      clear_filter = "<leader>cr",
      toggle_db_browser = "<leader>db",
      trigger_completion = "<C-Space>",
      help = "g?",
    },
    sql_dataset = {
      close = "q",
      move_left = "h",
      move_down = "j",
      move_up = "k",
      move_right = "l",
      prev_page = "H",
      next_page = "L",
      first_col = "0",
      last_col = "$",
      first_row = "gg",
      last_row = "G",
      preview_cell = "K",
      yank_cell = "yy",
      yank_column = "yc",
      sort_column = "s",
      toggle_cell_highlight = "zh",
      toggle_header_float = "zH",
      toggle_row_numbers = "zN",
      toggle_raw_mode = "<leader>gp",
      next_tab = "<Tab>",
      prev_tab = "<S-Tab>",
      rerun = "R",
      goto_first_page = "<leader>hh",
      goto_last_page = "<leader>ll",
      toggle_pagination = "<leader>pa",
      find_column = "<leader>fc",
      filter_by_cell = "<leader>ce",
      show_search = "<leader>/",
      clear_filter_search = "<leader>cr",
      next_search = "n",
      prev_search = "N",
      edit_cell = "i",
      edit_cell_replace = "cc",
      delete_row = "dd",
      insert_row = "o",
      commit_edits = "<leader>w",
      export = "E",
      help = "g?",
    },
    sql_table_ops = {
      select_all = "ma",
      refresh_all = "mr",
      describe_all = "md",
      toggle_menu = "mt",
    },
    sql_db_browser = {
      toggle_node = "<CR>",
      move_left = "h",
      move_right = "l",
      context_menu = "x",
      refresh_node = "r",
      search_filter = "/",
      select_query = "s",
      describe_query = "d",
      close = "q",
      search_next = "n",
      search_prev = "N",
      help = "g?",
    },
    sql_introspect = {
      close = "q",
      close_alt = "<Esc>",
    },
  },
  highlights = {},
}

M.current_env = M.config.default_env
M._sql_session = nil

local ok, sql_state = pcall(require, "poste.sql.state")
if ok then M.sql = sql_state end

local KEY_DISPLAY_NAMES = {
  ["<Tab>"] = "Tab",
  ["<S-Tab>"] = "S-Tab",
  ["<CR>"] = "Enter",
  ["<Esc>"] = "Esc",
  ["<Space>"] = "<Space>",
  ["<Up>"] = "Up",
  ["<Down>"] = "Down",
  ["<Left>"] = "Left",
  ["<Right>"] = "Right",
  ["<C-Space>"] = "C-Space",
  ["<BS>"] = "BS",
}

function M.get_keymap(section, action, default)
  local km = M.config.keymaps
  if not km then return default end
  local sec = km[section]
  if not sec then return default end
  local key = sec[action]
  if key == nil then return default end
  if key == false then return nil end
  return key
end

function M.format_key_string(key)
  if not key or key == "" then return "" end
  if KEY_DISPLAY_NAMES[key] then return KEY_DISPLAY_NAMES[key] end
  if key:sub(1, 8) == "<leader>" then
    local leader = vim.g.mapleader or "\\"
    if leader == " " then leader = "<Space>"
    elseif leader == "\t" then leader = "<Tab>"
    elseif leader == "\r" then leader = "<CR>"
    end
    leader = KEY_DISPLAY_NAMES[leader] or leader
    return leader .. key:sub(9)
  end
  return key
end

function M.format_keymap(section, action)
  local key = M.get_keymap(section, action)
  if not key then return "" end
  return M.format_key_string(key)
end

function M.find_poste_binary()
  local g_val = vim.g.poste_binary
  if g_val and g_val ~= "" and vim.fn.filereadable(g_val) == 1 then
    return vim.fn.fnamemodify(g_val, ":p")
  end
  if M.config.poste_binary ~= "" and vim.fn.filereadable(M.config.poste_binary) == 1 then
    return vim.fn.fnamemodify(M.config.poste_binary, ":p")
  end
  local paths = {}
  local cwd = vim.fn.getcwd()
  if cwd ~= "" then
    table.insert(paths, cwd .. "/target/debug/poste")
    table.insert(paths, cwd .. "/target/release/poste")
  end
  local src = debug.getinfo(M.find_poste_binary, "S").source
  if src:sub(1, 1) == "@" then
    local dir = src:sub(2):match("^(.+/)lua/poste/") or ""
    if dir ~= "" then
      table.insert(paths, dir .. "target/debug/poste")
      table.insert(paths, dir .. "target/release/poste")
      table.insert(paths, dir .. "bin/poste")
    end
  end
  for _, p in ipairs(paths) do
    if vim.fn.filereadable(p) == 1 then return vim.fn.fnamemodify(p, ":p") end
  end
  local path = vim.fn.exepath("poste")
  return path ~= "" and path or nil
end

function M.apply_highlight_overrides(group_names)
  local overrides = M.config.highlights
  if not overrides or vim.tbl_isempty(overrides) then return end
  for _, name in ipairs(group_names) do
    local attr = overrides[name]
    if attr then
      vim.api.nvim_set_hl(0, name, attr)
    end
  end
end

function M.log(level, msg)
  if not M.config.log_file or M.config.log_file == "" then return end
  local ts = os.date("%Y-%m-%d %H:%M:%S")
  local line = string.format("[%s] [%s] %s\n", ts, level, msg)
  local f = io.open(M.config.log_file, "a")
  if f then
    f:write(line)
    f:close()
  end
end

return M
