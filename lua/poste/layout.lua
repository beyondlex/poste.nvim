local M = {}

--- Pad text to a given display width (handles CJK via strdisplaywidth).
---@param text string
---@param width number Target display width
---@return string
local function pad(text, width)
  local dw = vim.fn.strdisplaywidth(text)
  local pad = math.max(0, width - dw)
  return text .. string.rep(" ", pad)
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
  local bar = "[" .. table.concat(bar_parts) .. string.rep(" ", bar_width - filled - (frac > 0 and 1 or 0)) .. "]"

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

return M