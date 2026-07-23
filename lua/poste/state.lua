--- Shared mutable state for the Poste plugin.
--- All modules require this to read/write cross-cutting state.
local C = require("poste.constants")
local M = {}

---------------------------------------------------------------------------
-- Configuration (defaults; replaced wholesale in setup via vim.tbl_deep_extend)
---------------------------------------------------------------------------
M.config = {
  poste_binary = vim.fn.stdpath("data") .. "/poste/bin/poste",
  default_env = "dev",
  split_direction = "vertical",
  split_size = 80,
  log_file = vim.fn.stdpath("cache") .. "/poste.log",
  import_chunk_size = 100,
  -- Response cache: large body files & binary response files saved here
  response_cache_dir = vim.fn.stdpath("cache") .. "/poste_res",
  -- Truncate large text responses: save to file, show preview + file link
  max_body_bytes = 100 * 1024,    -- 100 KB
  max_body_lines = 500,
  body_preview_lines = 20,

  -- Preferred SQL formatter order. If a formatter fails (e.g. sqlfluff can't
  -- parse SHOW TABLES), Poste automatically falls back to the next in the list.
  -- Available: "sqlfluff", "sqlfmt", "sql-formatter", "pg_format"
  -- Set to an empty list {} to disable formatting entirely.
  sql_formatters = { "sqlfluff", "sqlfmt", "sql-formatter", "pg_format" },

  -- User-overridable keymaps. Set to false to disable a keymap.
  -- Each section maps action names → key strings.
  keymaps = {
    http_source = {
      run = "<CR>",
      run_hsplit = "<M-CR>",
      jump_next = "]]",
      jump_prev = "[[",
      goto_definition = "gd",
      goto_references = "grr",
      quickfix_next = "]q",
      quickfix_prev = "[q",
      paste_curl = "<leader>rp",
      copy_as_curl = "<leader>rc",
      toggle_outline = "gs",
      pick_env = "<leader>vv",
      show_var_value = "K",
      show_history = "<leader>l",
      help = "g?",
    },
    http_response = {
      close = "q",
      view_body = "B",
      view_request = "R",
      view_verbose = "E",
      view_assertions = "A",
      view_script_logs = "S",
      next_tab = "<Tab>",
      prev_tab = "<S-Tab>",
      rerun = "r",
      next_response = "]",
      prev_response = "[",
      json_filter = "<leader>j",
      json_restore = "<leader>jc",
      json_toggle_raw = "<leader>jr",
      json_outline = "<leader>jo",
      image_preview = "K",
    },
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
    http_history = {
      close = "q",
      delete_entry = "dd",
      focus_detail = "<CR>",
    },
  },

  -- User-overridable highlight groups. Each key is a group name; value is
  -- a table of attributes (fg, bg, bold, italic, link, etc.) passed to
  -- vim.api.nvim_set_hl. Overrides are applied after default setup, so
  -- users only need to specify the attributes they want to change.
  highlights = {},
}

---------------------------------------------------------------------------
-- Cross-cutting mutable state
---------------------------------------------------------------------------
M.current_env = M.config.default_env
M.last_response = nil            -- parsed JSON table from --json output (request-scoped; cleared on session begin)
M.last_responses = nil           -- multi-response chain (request-scoped)
M.response_index = nil           -- index into last_responses (request-scoped)
M.last_assertion_results = nil   -- { tests, logs, total, passed, failed } (request-scoped)
M.last_script_logs = nil         -- { "log line 1", "log line 2", ... } (request-scoped)
M.last_request = nil             -- { buf, line } for re-run from response buffer
M.pending_request = nil          -- { method, url, headers_str, body, env, timestamp, start_hires } — in-flight (request-scoped)
M.current_view = "body"          -- "body" | "headers" | "verbose" | "assertions" | "script_logs"
M._split_override = nil          -- "vertical" | "horizontal" — override split direction for next render (cleared on use)

-- Active protocol sessions (Phase 2b). Created at each run_* entry; discarded on next begin.
-- Prefer reading/writing through session modules; these fields are the active references.
M._http_session = nil
M._sql_session = nil

-- HTTP request history (session-scoped across requests — intentional persistence)
M.http_history = {}              -- entry[] (newest first)
M.http_history_max = C.HTTP_HISTORY_MAX         -- max entries to keep
M.http_history_id_counter = 0    -- auto-increment ID

-- Script variable stores (persist across requests by design)
M.global_vars = {}               -- client.global.set/get persistence
M.script_variables = {}          -- request.variables from post-scripts (available to next request)

---------------------------------------------------------------------------
-- JSON response UX state (isolated from SQL; filter fields cleared per request)
---------------------------------------------------------------------------
M._json = {
  original_lines = nil,
  query = nil,
  is_filtered = false,
  pretty_mode = true,  -- user preference; not cleared per request
}

---------------------------------------------------------------------------
-- Deprecated write logging (Phase 2b)
-- When true, writes to request-scoped fields outside a session log a warning.
-- Enable via vim.g.poste_debug_state = true for lifecycle audits.
---------------------------------------------------------------------------
M._deprecated_write_log = false

local REQUEST_SCOPED = {
  last_response = true,
  last_responses = true,
  response_index = true,
  last_assertion_results = true,
  last_script_logs = true,
  pending_request = true,
}

--- Log a deprecation notice when request-scoped state is written without a session.
--- No-op unless vim.g.poste_debug_state or M._deprecated_write_log is set.
function M.deprecated_write(field, _value)
  if not (M._deprecated_write_log or vim.g.poste_debug_state) then return end
  if not REQUEST_SCOPED[field] then return end
  if M._http_session or M._sql_session then return end
  vim.notify(
    string.format("[poste] deprecated write to state.%s outside an active session", field),
    vim.log.levels.DEBUG,
    { title = "Poste" }
  )
end

-- SQL-specific state (loaded by poste-sql.nvim plugin when installed)
local ok, sql_state = pcall(require, "poste.sql.state")
if ok then M.sql = sql_state end

---------------------------------------------------------------------------
-- Keymap lookup helper
---------------------------------------------------------------------------
--- Look up a user-configured keymap from state.config.keymaps.
--- @param section string  e.g. "http_source", "sql_dataset"
--- @param action  string  e.g. "run", "close"
--- @param default string  fallback key if not configured
--- @return string|nil  the key to use, or nil if disabled (set to false)
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

--- Resolve a raw key string for display in UI labels.
--- <leader> is resolved via vim.g.mapleader; special chars are mapped to readable names.
function M.format_key_string(key)
  if not key or key == "" then return "" end
  if KEY_DISPLAY_NAMES[key] then return KEY_DISPLAY_NAMES[key] end
  if key:sub(1, 8) == "<leader>" then
    local leader = vim.g.mapleader or "\\"
    -- normalize literal whitespace chars to <> notation for KEY_DISPLAY_NAMES lookup
    if leader == " " then leader = "<Space>"
    elseif leader == "\t" then leader = "<Tab>"
    elseif leader == "\r" then leader = "<CR>"
    end
    leader = KEY_DISPLAY_NAMES[leader] or leader
    return leader .. key:sub(9)
  end
  return key
end

--- Look up a keymap and format it for display.
--- Returns empty string if keymap is disabled or not found.
function M.format_keymap(section, action)
  local key = M.get_keymap(section, action)
  if not key then return "" end
  return M.format_key_string(key)
end

---------------------------------------------------------------------------
-- Binary discovery
function M.find_poste_binary()
  -- g:poste_binary takes highest priority (quick override without touching config)
  local g_val = vim.g.poste_binary
  if g_val and g_val ~= "" and vim.fn.filereadable(g_val) == 1 then
    return vim.fn.fnamemodify(g_val, ":p")
  end
  if M.config.poste_binary ~= "" and vim.fn.filereadable(M.config.poste_binary) == 1 then
    return vim.fn.fnamemodify(M.config.poste_binary, ":p")
  end
  -- CWD-relative (works when nvim is launched from poste repo root)
  for _, path in ipairs(C.BINARY_CWD_PATHS) do
    if vim.fn.filereadable(path) == 1 then
      return vim.fn.fnamemodify(path, ":p")
    end
  end
  -- Plugin-relative (works when poste is on rtp from a repo checkout)
  local src = debug.getinfo(1, "S").source
  local root = src:sub(1, 1) == "@" and src:sub(2):match("^(.+/)lua/poste/")
  if root then
    for _, p in ipairs({ root .. "target/debug/poste", root .. "target/release/poste" }) do
      if vim.fn.filereadable(p) == 1 then
        return vim.fn.fnamemodify(p, ":p")
      end
    end
  end
  local path = vim.fn.exepath("poste")
  return path ~= "" and path or nil
end

-- Highlight overrides from user config
---------------------------------------------------------------------------
--- Apply user highlight overrides from state.config.highlights.
--- Call this at the end of each highlight setup() function.
--- @param group_names string[] List of group names to check for overrides
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

-- Logging
---------------------------------------------------------------------------
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
