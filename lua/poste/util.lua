--- Poste shared utility functions used across HTTP and SQL subsystems.
---
--- Extracted from init.lua and sql/init.lua to eliminate code duplication
--- and provide a single source of truth for common operations like JSON
--- cleanup, file discovery, and job data normalization.

local M = {}

---------------------------------------------------------------------------
-- vim.NIL cleanup
---------------------------------------------------------------------------

--- Recursively remove vim.NIL values from a parsed JSON table.
---
--- `vim.json.decode` converts JSON `null` to `vim.NIL`. This function
--- replaces those values with Lua `nil` so they can be safely compared
--- with `== nil`. It mutates the table in place and also returns it.
---
--- @param t table|nil  A table from vim.json.decode, or nil
--- @return table|nil    The same table with vim.NIL values removed, or nil
function M.clean_nil(t)
  if not t or type(t) ~= "table" then return t end
  for k, v in pairs(t) do
    if v == vim.NIL then
      t[k] = nil
    elseif type(v) == "table" then
      M.clean_nil(v)
    end
  end
  return t
end

---------------------------------------------------------------------------
-- File discovery (walk up directory tree)
---------------------------------------------------------------------------

--- Walk up the directory tree from `start_dir` to find a file.
--- Checks each ancestor directory for the given filename, stopping at
--- the filesystem root. Returns the first matching absolute path, or nil.
---
--- @param filename  string  File name to search for (e.g. "env.json")
--- @param start_dir string  Directory to start searching from
--- @return string|nil       Absolute path to the found file, or nil
function M.find_file_upwards(filename, start_dir)
  if not filename or filename == "" then return nil end
  local dir = start_dir or vim.fn.getcwd()
  while true do
    local candidate = dir .. "/" .. filename
    if vim.fn.filereadable(candidate) == 1 then
      return candidate
    end
    local parent = vim.fn.fnamemodify(dir, ":h")
    if parent == dir then
      return nil
    end
    dir = parent
  end
end

---------------------------------------------------------------------------
-- Job output data normalization
---------------------------------------------------------------------------

--- Given an array of lines from a job's stdout/stderr callback, remove
--- trailing empty strings. Neovim's jobstart callbacks often include
--- one or more trailing empty strings; this is a safe normalizer.
---
--- Returns the same array (mutated in place) with trailing empties
--- removed, for consistency with existing calling conventions.
---
--- @param data string[]|nil  Lines from a job callback
--- @return string[]          Cleaned array (may be empty)
function M.ensure_job_data(data)
  if not data or type(data) ~= "table" then return {} end
  while #data > 0 and data[#data] == "" do
    data[#data] = nil
  end
  return data
end

return M