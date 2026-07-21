--- Item selector using snacks.nvim.
---
--- Dependency: snacks.nvim (https://github.com/folke/snacks.nvim)
--- Falls back to built-in float → vim.ui.select if unavailable.
---
--- Items: string[] | { key, name, description }[]
---   - string: displayed as-is, key = value
---   - table:  key (for submission), name (display), description (secondary)
local M = {}

local function normalize_items(items)
  local result = {}
  for i, v in ipairs(items) do
    if type(v) == "string" then
      result[i] = { key = v, name = v, description = "" }
    elseif type(v) == "table" then
      result[i] = {
        key = v.key or v.name or tostring(i),
        name = v.name or v.key or tostring(i),
        description = v.description or "",
      }
    else
      result[i] = { key = tostring(v), name = tostring(v), description = "" }
    end
  end
  return result
end

local function pick_snacks(items, prompt, on_select)
  local picker_items = {}
  for _, item in ipairs(items) do
    picker_items[#picker_items + 1] = {
      text = item.name,
      description = item.description,
      key = item.key,
    }
  end

  local resolved = false
  Snacks.picker.select(
    picker_items,
    {
      prompt = prompt or 'Select items:',
      layout = 'select',
      format_item = function(item)
        local text = item.text
        if item.description and item.description ~= "" then
          text = text .. "  " .. item.description
        end
        return text
      end,
      close = function()
        if not resolved then
          resolved = true
          on_select(nil)
        end
      end,
    },
    function(item, idx)
      if not resolved then
        resolved = true
        on_select(item and item.key or nil)
      end
    end
  )
end

-- Fallback: built-in floating window
local function pick_float(items, prompt, on_select)
  local list_buf = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_buf_set_option(list_buf, "bufhidden", "wipe")
  vim.api.nvim_buf_set_option(list_buf, "filetype", "PosteSelect")

  local width = math.min(80, vim.o.columns - 4)
  local height = math.min(24, #items + 2)
  local row = math.floor((vim.o.lines - height) / 2)
  local col = math.floor((vim.o.columns - width) / 2)

  local win = vim.api.nvim_open_win(list_buf, true, {
    relative = "editor",
    width = width,
    height = height,
    row = row,
    col = col,
    style = "minimal",
    border = "rounded",
    title = prompt,
    title_pos = "center",
  })

  local selected_idx = 1
  local search_text = ""
  local filtered = vim.deepcopy(items)
  local resolved = false

  local function resolve(result)
    if resolved then return end
    resolved = true
    pcall(vim.api.nvim_win_close, win, true)
    vim.schedule(function() pcall(on_select, result) end)
  end

  local function render()
    local display = {}
    for _, item in ipairs(filtered) do
      local label = item.name
      if item.description ~= "" then
        label = label .. "  (" .. item.description .. ")"
      end
      table.insert(display, label)
    end
    local lines = { "\239\134\133 " .. search_text }
    for idx, label in ipairs(display) do
      local prefix = (idx == selected_idx) and "▶ " or "  "
      table.insert(lines, prefix .. label)
    end
    while #lines < height do table.insert(lines, "") end
    vim.api.nvim_buf_set_lines(list_buf, 0, -1, false, lines)
    vim.api.nvim_buf_clear_namespace(list_buf, -1, 0, -1)
    if selected_idx > 0 and selected_idx <= #filtered then
      vim.api.nvim_buf_add_highlight(list_buf, -1, "Visual", selected_idx, 0, -1)
    end
  end

  local function filter_items()
    filtered = {}
    if search_text == "" then
      filtered = vim.deepcopy(items)
      selected_idx = 1
    else
      local lower = search_text:lower()
      for _, item in ipairs(items) do
        if item.name:lower():find(lower, 1, true)
          or item.description:lower():find(lower, 1, true) then
          table.insert(filtered, item)
        end
      end
      selected_idx = 1
    end
    render()
  end

  local function map(mode, key, action)
    vim.keymap.set(mode, key, action, { buffer = list_buf, nowait = true })
  end

  map("n", "j",      function() selected_idx = math.min(selected_idx + 1, #filtered); render() end)
  map("n", "k",      function() selected_idx = math.max(selected_idx - 1, 1);         render() end)
  map("n", "<Down>", function() selected_idx = math.min(selected_idx + 1, #filtered); render() end)
  map("n", "<Up>",   function() selected_idx = math.max(selected_idx - 1, 1);         render() end)
  map("n", "<CR>", function() resolve(#filtered > 0 and filtered[selected_idx].key or nil) end)
  map("n", "<Esc>", function() resolve(nil) end)
  map("n", "q",     function() resolve(nil) end)
  map("n", "i",     function() vim.cmd("startinsert!") end)
  map("n", "a",     function() vim.cmd("startinsert!") end)

  map("i", "<CR>",   function() vim.cmd("stopinsert"); resolve(#filtered > 0 and filtered[selected_idx].key or nil) end)
  map("i", "<Esc>",  function() vim.cmd("stopinsert"); resolve(nil) end)
  map("i", "<Down>", function() selected_idx = math.min(selected_idx + 1, #filtered); render() end)
  map("i", "<Up>",   function() selected_idx = math.max(selected_idx - 1, 1);         render() end)

  vim.api.nvim_create_autocmd("TextChangedI", {
    buffer = list_buf,
    callback = function()
      if resolved then return end
      local lines = vim.api.nvim_buf_get_lines(list_buf, 0, 1, false)
      local new_search = (lines[1] or ""):match("^\239\134\133 (.*)$") or ""
      if new_search ~= search_text then
        search_text = new_search
        filter_items()
      end
    end,
  })

  render()
  vim.cmd("startinsert!")
end

-- Last resort: vim.ui.select
local function pick_vimui(items, prompt, on_select)
  vim.ui.select(items, {
    prompt = prompt,
    format_item = function(item)
      if item.description ~= "" then
        return item.name .. "  (" .. item.description .. ")"
      end
      return item.name
    end,
  }, function(choice)
    on_select(choice and choice.key or nil)
  end)
end

--- Show a picker and call on_select(key) with the selected item's key (nil on cancel).
--- @param items string[] | {key, name, description}[]
--- @param prompt string  Title
--- @param on_select fun(key: string|nil)
function M.select(items, prompt, on_select)
  local normalized = normalize_items(items)
  if #normalized == 0 then
    vim.schedule(function() pcall(on_select, nil) end)
    return
  end

  local ok, _ = pcall(require, "snacks.picker")
  if ok then
    pick_snacks(normalized, prompt, on_select)
    return
  end

  local ok, _ = pcall(pick_float, normalized, prompt, on_select)
  if ok then return end

  pick_vimui(normalized, prompt, on_select)
end

return M
