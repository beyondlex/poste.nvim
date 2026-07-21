--- Unified error handling for Poste.
---
--- Centralizes all user-facing notifications and log writes.
--- Every module should call M.notify() instead of vim.notify directly.
---
--- Error levels:
---   DEBUG   — verbose diagnostics (hidden from user by default)
---   INFO    — normal status messages
---   WARN    — recoverable issues
---   ERROR   — non-recoverable errors

local state = require("poste.state")

local M = {}

local LEVELS = {
  DEBUG = vim.log.levels.DEBUG,
  INFO  = vim.log.levels.INFO,
  WARN  = vim.log.levels.WARN,
  ERROR = vim.log.levels.ERROR,
}

--- Notify the user AND write to the log file.
--- @param msg string  The message
--- @param level string|number  "DEBUG" | "INFO" | "WARN" | "ERROR" or vim.log.levels.*
--- @param opts table|nil  Options passed to vim.notify (title, icon, etc.)
function M.notify(msg, level, opts)
  opts = opts or {}
  -- Resolve string level names to numeric values
  local lvl = type(level) == "string" and (LEVELS[level:upper()] or vim.log.levels.INFO) or level or vim.log.levels.INFO
  local lvl_name = level
  if type(level) == "number" then
    for k, v in pairs(LEVELS) do
      if v == level then lvl_name = k; break end
    end
  end
  -- Always write to log
  state.log(lvl_name or "INFO", msg)
  -- Skip DEBUG for user notification
  if lvl == vim.log.levels.DEBUG then return end
  -- Show to user
  vim.notify(msg, lvl, opts)
end

--- Convenience wrappers
function M.debug(msg, opts)  M.notify(msg, "DEBUG", opts) end
function M.info(msg, opts)   M.notify(msg, "INFO", opts) end
function M.warn(msg, opts)   M.notify(msg, "WARN", opts) end
function M.error(msg, opts)  M.notify(msg, "ERROR", opts) end

return M
