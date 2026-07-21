local state = require("poste.state")

local M = {}

local DESCRIPTIONS = {
  http_source = {
    run = "Execute request under cursor",
    run_hsplit = "Execute request, horizontal split",
    jump_next = "Jump to next request block",
    jump_prev = "Jump to previous request block",
    goto_definition = "Go to variable definition",
    goto_references = "Show variable references",
    quickfix_next = "Next quickfix item",
    quickfix_prev = "Previous quickfix item",
    paste_curl = "Paste clipboard as cURL request",
    copy_as_curl = "Copy request as cURL command",
    toggle_outline = "Toggle outline window",
    pick_env = "Pick environment",
    show_var_value = "Show variable value / response chain",
    show_history = "Open request history",
    help = "Show this help window",
  },
  http_response = {
    close = "Close response window",
    rerun = "Re-run request",
    view_body = "View response body",
    view_verbose = "View verbose output",
    view_assertions = "View assertion results",
    view_script_logs = "View pre/post script logs",
    next_tab = "Next response tab",
    prev_tab = "Previous response tab",
    image_preview = "Render image inline or open externally",
  },
  http_history = {
    close = "Close history window",
    delete_entry = "Delete current history entry",
    focus_detail = "Focus detail pane",
  },
  sql_source = {
    run = "Execute SQL statement",
    show_ddl = "Show DDL / column info",
    goto_definition = "Go to connection/database/table definition",
    format = "Format SQL",
    clear_filter = "Clear filter / search",
    toggle_db_browser = "Toggle DB Browser panel",
    trigger_completion = "Trigger SQL completion",
    toggle_log = "Toggle execution log",
    help = "Show this help window",
  },
  sql_dataset = {
    close = "Close dataset window",
    move_left = "Move cell left",
    move_down = "Move cell down",
    move_up = "Move cell up",
    move_right = "Move cell right",
    prev_page = "Previous page",
    next_page = "Next page",
    first_col = "Jump to first column",
    last_col = "Jump to last column",
    first_row = "Jump to first row",
    last_row = "Jump to last row",
    preview_cell = "Preview cell content",
    yank_cell = "Yank current cell",
    yank_column = "Yank current column",
    sort_column = "Sort by column",
    toggle_cell_highlight = "Toggle cell highlight",
    toggle_header_float = "Toggle floating header",
    toggle_row_numbers = "Toggle row numbers",
    toggle_raw_mode = "Toggle raw table mode",
    next_tab = "Next result tab",
    prev_tab = "Previous result tab",
    rerun = "Re-run query",
    goto_first_page = "Go to first page",
    goto_last_page = "Go to last page",
    toggle_pagination = "Toggle pagination",
    find_column = "Find column",
    filter_by_cell = "Filter by cell value",
    show_search = "Search in results",
    clear_filter_search = "Clear filter / search",
    next_search = "Next search match",
    prev_search = "Previous search match",
    commit_edits = "Commit pending edits",
    edit_cell = "Edit cell value",
    edit_cell_replace = "Replace cell value",
    delete_row = "Delete row",
    insert_row = "Insert row",
    export = "Export dataset (format -> destination)",
  },
  sql_table_ops = {
    select_all = "SELECT * from table",
    refresh_all = "Refresh table list",
    describe_all = "DESCRIBE table",
    toggle_menu = "Toggle action menu",
  },
  sql_db_browser = {
    toggle_node = "Toggle expand/collapse node",
    move_left = "Collapse node / go to parent",
    move_right = "Expand node / go to first child",
    context_menu = "Open context menu",
    refresh_node = "Refresh node children",
    search_filter = "Fuzzy search tree",
    select_query = "Generate SELECT query",
    describe_query = "Generate DESCRIBE query",
    close = "Close DB Browser",
    search_next = "Next search match",
    search_prev = "Previous search match",
  },
  sql_introspect = {
    close = "Close introspect window",
    close_alt = "Close introspect window",
  },
}

local SECTION_TITLES = {
  http_source = "HTTP Request Buffer",
  http_response = "HTTP Response Buffer",
  http_history = "HTTP Request History",
  sql_source = "SQL Source Buffer",
  sql_dataset = "SQL Dataset Buffer",
  sql_table_ops = "SQL Table Ops",
  sql_db_browser = "DB Browser",
  sql_introspect = "Introspect Float",
}

local function sections_for_filetype(ft)
  if ft == "poste_sql" or ft == "poste_sqlite" then
    return { "sql_source", "sql_dataset", "sql_table_ops", "sql_db_browser", "sql_introspect" }
  end
  return { "http_source", "http_response", "http_history" }
end

function M.open()
  local lines = {}
  local width = 50
  local ft = vim.bo.filetype

  for _, section in ipairs(sections_for_filetype(ft)) do
    local title = SECTION_TITLES[section] or section
    local km = state.config.keymaps[section] or {}
    local desc = DESCRIPTIONS[section] or {}

    table.insert(lines, "")
    table.insert(lines, "  " .. title)
    table.insert(lines, "  " .. string.rep("─", 46))

    local actions = {}
    for action, _ in pairs(km) do
      table.insert(actions, action)
    end
    table.sort(actions)

    for _, action in ipairs(actions) do
      local key = state.get_keymap(section, action)
      if key and key ~= false then
        local key_display = state.format_key_string(key)
        local description = desc[action] or ""
        local line = string.format("  %-12s  %s", key_display, description)
        table.insert(lines, line)
        width = math.max(width, #line + 2)
      end
    end
  end

  local close_keys = {}
  local function collect_close(section, action)
    local k = state.get_keymap(section, action)
    if k then close_keys[state.format_key_string(k)] = true end
  end
  if ft == "poste_sql" or ft == "poste_sqlite" then
    collect_close("sql_dataset", "close")
    collect_close("sql_db_browser", "close")
    collect_close("sql_introspect", "close")
    collect_close("sql_introspect", "close_alt")
  else
    collect_close("http_response", "close")
    collect_close("http_history", "close")
  end
  local close_parts = {}
  for k in pairs(close_keys) do
    table.insert(close_parts, k)
  end
  table.sort(close_parts, function(a, b) return #a < #b end)
  local close_text = #close_parts > 0 and table.concat(close_parts, " / ") or "q"
  table.insert(lines, "  " .. close_text .. "  close")

  local buf = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_buf_set_lines(buf, 0, -1, false, lines)
  vim.bo[buf].modifiable = false
  vim.bo[buf].bufhidden = "wipe"
  vim.bo[buf].buftype = "nofile"
  vim.bo[buf].filetype = "poste_help"

  local height = math.min(#lines, vim.o.lines - 4)
  local win = vim.api.nvim_open_win(buf, true, {
    relative = "editor",
    row = 2, col = math.floor((vim.o.columns - width) / 2),
    width = width, height = height,
    style = "minimal",
    border = "rounded",
    title = " Poste Keymaps ",
    title_pos = "center",
  })

  vim.keymap.set("n", "q", function() pcall(vim.api.nvim_win_close, win, true) end, { buffer = buf, nowait = true })
  vim.keymap.set("n", "<Esc>", function() pcall(vim.api.nvim_win_close, win, true) end, { buffer = buf, nowait = true })

  local ns = vim.api.nvim_create_namespace("poste_help")
  for i, line in ipairs(lines) do
    if line:find("^  %u%a") then
      vim.api.nvim_buf_add_highlight(buf, ns, "Title", i - 1, 2, -1)
    elseif line:find("^  ─") then
      vim.api.nvim_buf_add_highlight(buf, ns, "Comment", i - 1, 2, -1)
    else
      local key_s, key_e = line:find("%S+", 3)
      if key_s then
        vim.api.nvim_buf_add_highlight(buf, ns, "Special", i - 1, key_s - 1, key_e)
      end
    end
  end
end

return M