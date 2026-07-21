--- Centralised CLI wrapper for `poste` binary invocations.
---
--- Every Lua module that needs to call the poste CLI should go through
--- this module. This ensures consistent error handling, JSON parsing,
--- and argument construction.

local M = {}

local state = nil
local function get_state()
  if not state then state = require("poste.state") end
  return state
end

--- Get the poste binary path, or nil if not found.
--- @return string|nil
function M.binary()
  return get_state().find_poste_binary()
end

--- Run a poste CLI command synchronously.
---
--- @param cmd table  List of arguments (excluding the binary itself).
---                  e.g. `{"run", "--json", ...}`
--- @param opts? table
---   - stdin: string|nil  Content to pipe to stdin
---   - binary: string|nil  Override binary path (default: M.binary())
--- @return string|nil, string|nil  stdout, error_message
function M.run(cmd, opts)
  opts = opts or {}
  local binary = opts.binary or M.binary()
  if not binary then
    return nil, "Poste binary not found"
  end

  local args = vim.list_extend({ binary }, cmd)
  local ok, result = pcall(vim.fn.system, args, opts.stdin)
  if not ok then
    return nil, tostring(result)
  end
  if vim.v.shell_error ~= 0 then
    return nil, result
  end
  return result, nil
end

--- Run a poste CLI command and parse JSON output.
---
--- @param cmd table  List of arguments
--- @param opts? table  Same as M.run()
--- @return table|nil, string|nil  parsed JSON table, error_message
function M.run_json(cmd, opts)
  local output, err = M.run(cmd, opts)
  if not output then
    return nil, err
  end
  local ok, parsed = pcall(vim.json.decode, output)
  if not ok then
    return nil, "Failed to parse JSON output: " .. tostring(parsed)
  end
  return parsed, nil
end

--- Run a poste CLI command asynchronously via jobstart.
---
--- @param cmd table  List of arguments
--- @param opts table
---   - on_stdout: function(lines)  Called with stdout lines
---   - on_stderr: function(lines)  Called with stderr lines
---   - on_exit: function(code)     Called on exit
---   - stdin: string|nil           Content to write to stdin
---   - binary: string|nil          Override binary path
--- @return number|nil  job_id, or nil on error
function M.run_async(cmd, opts)
  opts = opts or {}
  local binary = opts.binary or M.binary()
  if not binary then
    if opts.on_exit then opts.on_exit(-1) end
    return nil
  end

  local args = vim.list_extend({ binary }, cmd)
  local job_opts = {
    stdout_buffered = true,
    stderr_buffered = true,
  }

  if opts.on_stdout then
    job_opts.on_stdout = function(_, data)
      opts.on_stdout(data or {})
    end
  end

  if opts.on_stderr then
    job_opts.on_stderr = function(_, data)
      opts.on_stderr(data or {})
    end
  end

  if opts.on_exit then
    job_opts.on_exit = function(_, code)
      opts.on_exit(code)
    end
  end

  local job_id = vim.fn.jobstart(args, job_opts)

  if opts.stdin and job_id > 0 then
    vim.fn.chansend(job_id, opts.stdin)
    vim.fn.chanclose(job_id, "stdin")
  end

  return job_id
end

return M