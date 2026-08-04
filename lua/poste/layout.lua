local M = {}

local state = nil
local function get_state()
  if not state then state = require("poste.state") end
  return state
end

local function setup_hl()
  vim.api.nvim_set_hl(0, "PosteCoreLayoutSectionTitle", { fg = 0x7dcfff, bold = true })
  vim.api.nvim_set_hl(0, "PosteCoreLayoutParagraph",   { fg = 0x565f89 })
  vim.api.nvim_set_hl(0, "PosteCoreLayoutKey",          { fg = 0x98c379, bold = true })
  vim.api.nvim_set_hl(0, "PosteCoreLayoutValue",        { fg = 0xa9b1d6 })
  get_state().apply_highlight_overrides({
    "PosteCoreLayoutSectionTitle", "PosteCoreLayoutParagraph",
    "PosteCoreLayoutKey", "PosteCoreLayoutValue",
  })
end
setup_hl()
vim.api.nvim_create_autocmd("ColorScheme", { callback = setup_hl })

local function word_wrap(text, max_width)
  if #text <= max_width then return { text } end
  local lines = {}
  while #text > 0 do
    if #text <= max_width then
      table.insert(lines, text)
      break
    end
    local slice = text:sub(1, max_width)
    local space_pos = slice:match("^.*()%s")
    if space_pos then
      table.insert(lines, text:sub(1, space_pos - 1))
      text = text:sub(space_pos + 1):match("^%s*(.*)")
    else
      table.insert(lines, text:sub(1, max_width))
      text = text:sub(max_width + 1)
    end
  end
  return lines
end

--- Word-wrap text at spaces, falling back to hard break.
---@param text string
---@param max_width number
---@return string[]
function M.word_wrap(text, max_width)
  return word_wrap(text, max_width)
end

--- Pad text to a given display width (handles CJK via strdisplaywidth).
---@param text string
---@param width number Target display width
---@return string
local function pad(text, width)
  local dw = vim.fn.strdisplaywidth(text)
  local padding = math.max(0, width - dw)
  return text .. string.rep(" ", padding)
end
M.pad = pad

--- Left-aligned cell with fixed width. Pads with spaces if text is shorter, no truncation.
---@param text string
---@param width number
---@return string
function M.cell(text, width)
  local s = tostring(text)
  return pad(s, width)
end

--- Dynamic line that fills a container width. Truncates with ellipsis if text exceeds.
---@param opts table
---  - text: string (required)
---  - container_width: number (required)
---  - padding: { left?: number, right?: number } (optional)
---  - ellipsis: string (optional, default "...")
---  - truncate_at: "left"|"mid"|"right" (optional, default "right")
---@return string
function M.dynamic_line(opts)
  local text = opts.text or ""
  local cw = opts.container_width or 60
  local pad_left = (opts.padding and opts.padding.left) or 0
  local pad_right = (opts.padding and opts.padding.right) or 0
  local ellipsis = opts.ellipsis or "..."
  local truncate_at = opts.truncate_at or "right"
  local content_width = cw - pad_left - pad_right
  if content_width <= 0 then return string.rep(" ", cw) end

  local s = tostring(text)
  local dw = vim.fn.strdisplaywidth(s)
  if dw > content_width then
    local el_dw = vim.fn.strdisplaywidth(ellipsis)
    local avail = content_width - el_dw
    if truncate_at == "left" then
      local start_byte = vim.fn.strcharpart(s, #s - avail, avail)
      return string.rep(" ", pad_left) .. ellipsis .. start_byte .. string.rep(" ", pad_right)
    elseif truncate_at == "mid" then
      local half = math.floor(avail / 2)
      local left_part = vim.fn.strcharpart(s, 0, half)
      local right_part = vim.fn.strcharpart(s, #s - (avail - half), avail - half)
      return string.rep(" ", pad_left) .. left_part .. ellipsis .. right_part .. string.rep(" ", pad_right)
    else
      local truncated = vim.fn.strcharpart(s, 0, avail)
      return string.rep(" ", pad_left) .. truncated .. ellipsis .. string.rep(" ", pad_right)
    end
  end
  return string.rep(" ", pad_left) .. s .. string.rep(" ", content_width - dw) .. string.rep(" ", pad_right)
end

--- Left/right alignment on the same line.
---@param left string Left-aligned text
---@param right string Right-aligned text
---@param opts? { width?: number }
---@return string[]
function M.space_between(left, right, opts)
  opts = opts or {}
  local width = opts.width or 60
  local left_dw = vim.fn.strdisplaywidth(left)
  local right_dw = vim.fn.strdisplaywidth(right)
  local gap = math.max(0, width - left_dw - right_dw)
  return { left .. string.rep(" ", gap) .. right }
end

--- Multi-column layout. Each column has a title and items.
--- Columns are equally sized; the last column absorbs rounding.
---@param cols table[] { title: string, items: string[] }[]
---@param opts? { width?: number, title_hl?: string }
---@return { lines: string[], highlights: table[] }
function M.columns(cols, opts)
  opts = opts or {}
  local width = opts.width or 60
  local n = #cols
  if n == 0 then return { lines = {}, highlights = {} } end

  local col_width = math.floor(width / n)
  local last_col_width = width - col_width * (n - 1)

  local max_rows = 0
  for _, col in ipairs(cols) do
    local nrows = #(col.items or {})
    if col.title then nrows = nrows + 1 end
    max_rows = math.max(max_rows, nrows)
  end

  local lines = {}
  local highlights = {}
  local title_hl = opts.title_hl or "Title"

  for i = 1, max_rows do
    local parts = {}
    for j, col in ipairs(cols) do
      local cw = j == n and last_col_width or col_width
      local item
      local is_title = false
      if i == 1 and col.title then
        item = col.title
        is_title = true
      elseif col.title then
        item = (col.items or {})[i - 1] or ""
      else
        item = (col.items or {})[i] or ""
      end
      local padded = M.cell(item, cw)
      table.insert(parts, padded)
      if is_title then
        local offset = 0
        for k = 1, j - 1 do
          local kw = k == n and last_col_width or col_width
          offset = offset + kw + 1
        end
        local dw = vim.fn.strdisplaywidth(item)
        table.insert(highlights, {
          line = i - 1,
          col_start = offset,
          col_end = offset + dw,
          hl_group = title_hl,
        })
      end
    end
    table.insert(lines, table.concat(parts, " "))
  end

  return { lines = lines, highlights = highlights }
end

--- Render a progress bar. Returns bar + label without padding to total_width.
---@param current number
---@param total number
---@param opts? { bar_width?: number }
---@return string[]
function M.progress(current, total, opts)
  opts = opts or {}
  local bar_width = opts.bar_width or 20
  local pct = total > 0 and (current / total) or 0
  local filled = math.floor(pct * bar_width)
  local frac = math.floor((pct * bar_width - filled) * 8)

  local bar_parts = {}
  for _ = 1, filled do
    table.insert(bar_parts, "█")
  end
  if frac > 0 and filled < bar_width then
    local bar_chars = { " ", "▏", "▎", "▍", "▌", "▋", "▊", "▉", "█" }
    table.insert(bar_parts, bar_chars[frac + 1])
  end
  local bar = table.concat(bar_parts) .. string.rep("░", bar_width - filled - (frac > 0 and 1 or 0))

  local label = string.format("%d/%d %d%%", current, total, math.floor(pct * 100))
  return { bar .. " " .. label }
end

--- Separator line.
---@param opts? { width?: number, char?: string }
---@return string[]
function M.separator(opts)
  opts = opts or {}
  local width = opts.width or 60
  local char = opts.char or "─"
  return { string.rep(char, width) }
end

--- Section title with highlight.
---@param opts table
---  - text: string
---  - indent: number? (default 2)
---  - hl: string? (default "PosteCoreLayoutSectionTitle", highlight group name)
---@return { lines: string[], highlights: table[] }
function M.section_title(opts)
  opts = opts or {}
  local text = opts.text or ""
  local indent = opts.indent or 2
  local hl = opts.hl or "PosteCoreLayoutSectionTitle"
  local prefix = string.rep(" ", indent)
  local line = prefix .. text
  local dw = vim.fn.strdisplaywidth(text)
  return {
    lines = { line },
    highlights = { { line = 0, col_start = indent, col_end = indent + dw, hl_group = hl } },
  }
end

--- Paragraph with optional word-wrap.
---@param opts table
---  - text: string|string[] (single string or array of lines)
---  - max_width: number (container width)
---  - indent: number? (default 4)
---  - auto_wrap: boolean? (default true)
---  - hl: string? (default "PosteCoreLayoutParagraph", highlight group name)
---@return { lines: string[], highlights: table[] }
function M.paragraph(opts)
  opts = opts or {}
  local text = opts.text or ""
  local max_width = opts.max_width or 60
  local indent = opts.indent or 4
  local auto_wrap = opts.auto_wrap ~= false
  local hl = opts.hl or "PosteCoreLayoutParagraph"
  local prefix = string.rep(" ", indent)
  local max_line = max_width - indent

  local lines = {}
  local highlights = {}
  local texts = type(text) == "table" and text or { text }

  for _, t in ipairs(texts) do
    local raw = tostring(t)
    if auto_wrap then
      local wrapped = word_wrap(raw, max_line)
      for _, wl in ipairs(wrapped) do
        local li = #lines
        table.insert(lines, prefix .. wl)
        table.insert(highlights, { line = li, col_start = 0, col_end = #lines[li + 1], hl_group = hl })
      end
    else
      local li = #lines
      table.insert(lines, prefix .. raw)
      table.insert(highlights, { line = li, col_start = 0, col_end = #lines[li + 1], hl_group = hl })
    end
  end

  return { lines = lines, highlights = highlights }
end

--- Keymap hint bar. Each entry is rendered as `[key label]`.
---@param opts table
---  - mapping: { key: string, label: string }[] (ordered list of key-label pairs)
---  - key_hl: string? (default "PosteCoreLayoutKey", highlight group for key)
---  - value_hl: string? (default "PosteCoreLayoutValue", highlight group for label)
---  - indent: number? (default 4)
---  - sep: string? (default "  ", separator between entries)
---@return { lines: string[], highlights: table[] }
function M.keymaps(opts)
  opts = opts or {}
  local mapping = opts.mapping or {}
  local key_hl = opts.key_hl or "PosteCoreLayoutKey"
  local value_hl = opts.value_hl or "PosteCoreLayoutValue"
  local indent = opts.indent or 4
  local sep = opts.sep or "  "
  local prefix = string.rep(" ", indent)

  local parts = { prefix }
  local highlights = {}
  local byte_pos = #prefix

  for _, entry in ipairs(mapping) do
    local key = entry.key or ""
    local label = entry.label or ""
    local segment = "[" .. key .. " " .. label .. "]"
    table.insert(parts, segment)
    local key_start = byte_pos + 1
    local key_end = key_start + #key
    table.insert(highlights, { line = 0, col_start = key_start, col_end = key_end, hl_group = key_hl })
    table.insert(highlights, { line = 0, col_start = key_end + 1, col_end = key_end + 1 + #label, hl_group = value_hl })
    byte_pos = byte_pos + #segment
    table.insert(parts, sep)
    byte_pos = byte_pos + #sep
  end

  local line = table.concat(parts)
  line = line:sub(1, -#sep - 1)

  return { lines = { line }, highlights = highlights }
end

return M
