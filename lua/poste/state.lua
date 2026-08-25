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
  highlights = {},
}

M.current_env = M.config.default_env

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
