local M = {}

local ns = vim.api.nvim_create_namespace("poste_dialog_hl")

local backdrop_refcount = 0
local backdrop_buf = nil
local backdrop_win = nil

local function setup_hl()
  vim.api.nvim_set_hl(0, "PosteCoreBackdrop", { bg = 0x000000, blend = 60 })
  vim.api.nvim_set_hl(0, "PosteCoreProgressBar", { fg = 0x5c6370 })
end
setup_hl()
vim.api.nvim_create_autocmd("ColorScheme", { callback = setup_hl })

local function ensure_backdrop()
  backdrop_refcount = backdrop_refcount + 1
  if backdrop_buf and vim.api.nvim_buf_is_valid(backdrop_buf) then
    return
  end
  backdrop_buf = vim.api.nvim_create_buf(false, true)
  vim.bo[backdrop_buf].bufhidden = "wipe"
  vim.bo[backdrop_buf].buftype = "nofile"

  backdrop_win = vim.api.nvim_open_win(backdrop_buf, false, {
    relative = "editor",
    width = vim.o.columns,
    height = vim.o.lines,
    row = 0,
    col = 0,
    style = "minimal",
    zindex = 49,
    focusable = false,
  })
  vim.wo[backdrop_win].winhl = "Normal:PosteCoreBackdrop"
  vim.wo[backdrop_win].winblend = 60
end

local function release_backdrop()
  backdrop_refcount = backdrop_refcount - 1
  if backdrop_refcount <= 0 then
    if backdrop_win and vim.api.nvim_win_is_valid(backdrop_win) then
      vim.api.nvim_win_close(backdrop_win, true)
    end
    backdrop_win = nil
    backdrop_buf = nil
  end
end

--- Open a dialog floating window.
---@param opts table
---  - title: string (optional, centered)
---  - width: number (default 60)
---  - height: number (default 10)
---  - border: string|false (default "rounded", false/"none" for no border)
---  - backdrop: boolean (default false, dim outside area)
---  - close_on_leave: boolean (default true, close dialog on WinLeave)
---  - on_close: function (optional, called when dialog closes)
---  - keymaps: { [key] = function } (optional, additional keymaps)
---@return table Dialog handle with .buf, .win, .content_width, .content_height
function M.open(opts)
  opts = opts or {}
  local width = opts.width or 60
  local height = opts.height or 10
  local has_border = opts.border ~= false and opts.border ~= "none"
  local border = has_border and (opts.border or "rounded") or "none"

  if opts.backdrop then
    ensure_backdrop()
  end

  local buf = vim.api.nvim_create_buf(false, true)
  vim.bo[buf].bufhidden = "wipe"
  vim.bo[buf].buftype = "nofile"
  vim.bo[buf].modifiable = false

  local row = math.floor((vim.o.lines - height) / 2)
  local col = math.floor((vim.o.columns - width) / 2)

  local win_opts = {
    relative = "editor",
    width = width,
    height = height,
    row = row,
    col = col,
    style = "minimal",
    border = border,
    zindex = 50,
  }
  if opts.title then
    win_opts.title = opts.title
    win_opts.title_pos = "center"
  end

  local win = vim.api.nvim_open_win(buf, true, win_opts)
  vim.wo[win].winhl = "Normal:NormalFloat"

  local self = {
    buf = buf,
    win = win,
    width = width,
    height = height,
    content_width = has_border and (width - 2) or width,
    content_height = has_border and (height - 2) or height,
    _has_backdrop = opts.backdrop or false,
    _on_close = opts.on_close,
    _closing = false,
  }

  local km = { buffer = buf, noremap = true, silent = true, nowait = true }
  vim.keymap.set("n", "q", function()
    self:close()
  end, km)
  vim.keymap.set("n", "<Esc>", function()
    self:close()
  end, km)

  if opts.keymaps then
    for key, fn in pairs(opts.keymaps) do
      vim.keymap.set("n", key, fn, km)
    end
  end

  if opts.close_on_leave ~= false then
    vim.api.nvim_create_autocmd("WinLeave", {
      buffer = buf,
      once = true,
      callback = function()
        self:close()
      end,
    })
  end

  return setmetatable(self, { __index = M })
end

--- Update dialog content.
---@param lines string[] Content lines (without border characters)
---@param highlights? table[] Optional highlight specs:
---   { line, col_start, col_end, hl_group }
function M:update(lines, highlights)
  if not self.buf or not vim.api.nvim_buf_is_valid(self.buf) then return end
  vim.bo[self.buf].modifiable = true
  vim.api.nvim_buf_set_lines(self.buf, 0, -1, false, lines or {})
  vim.bo[self.buf].modifiable = false

  vim.api.nvim_buf_clear_namespace(self.buf, ns, 0, -1)
  if highlights then
    for _, h in ipairs(highlights) do
      local end_col = h.col_end
      if end_col == -1 then end_col = 99999 end
      vim.api.nvim_buf_set_extmark(self.buf, ns, h.line, h.col_start, {
        end_col = end_col,
        hl_group = h.hl_group,
      })
    end
  end
  vim.cmd("redraw")
end

--- Close the dialog.
function M:close()
  if self._closing then return end
  self._closing = true
  if self._on_close then
    self._on_close()
  end
  if self.win and vim.api.nvim_win_is_valid(self.win) then
    vim.api.nvim_win_close(self.win, true)
  end
  self.win = nil
  self.buf = nil
  if self._has_backdrop then
    release_backdrop()
  end
end

return M