--- Lightweight Promise/A+ implementation for flattening callback chains.
---
--- Converts nested callbacks into a linear chain:
---
---   local P = require("poste.async.promise")
---
---   P.new(function(resolve, reject)
---     async_operation(function(result)
---       resolve(result)
---     end)
---   end)
---   :then_(function(result)
---     return P.new(function(resolve)
---       another_async_op(result, resolve)
---     end)
---   end)
---   :then_(function(result)
---     print("Final:", result)
---   end)
---   :catch_(function(err)
---     vim.notify("Error: " .. err, vim.log.levels.ERROR)
---   end)

local M = {}

local Promise = {}
Promise.__index = Promise

--- Create a new Promise.
--- @param resolve_fn function(resolve, reject)  Executor for resolve path (or nil)
--- @param reject_fn function(reject)            Executor for reject path (optional)
--- @return Promise
function M.new(resolve_fn, reject_fn)
  local self = setmetatable({
    _state = "pending",  -- "pending" | "fulfilled" | "rejected"
    _value = nil,
    _handlers = {},
  }, Promise)

  local function resolve(value)
    if self._state ~= "pending" then return end
    self._state = "fulfilled"
    self._value = value
    self:_call_handlers()
  end

  local function reject(err)
    if self._state ~= "pending" then return end
    self._state = "rejected"
    self._value = err
    self:_call_handlers()
  end

  if reject_fn then
    -- Two-argument form: resolve_fn is called with resolve, reject_fn is called with reject
    if resolve_fn then
      pcall(resolve_fn, resolve, reject)
    end
    pcall(reject_fn, reject)
  elseif resolve_fn then
    local ok, err = pcall(resolve_fn, resolve, reject)
    if not ok then
      reject(err)
    end
  end

  return self
end

--- Register fulfillment handler. Returns a new Promise for chaining.
--- @param on_fulfilled function(value)  Called when promise is fulfilled
--- @return Promise
function Promise:then_(on_fulfilled)
  return M.new(function(resolve, reject)
    table.insert(self._handlers, {
      on_fulfilled = function(value)
        local ok, result = pcall(on_fulfilled, value)
        if ok then
          if type(result) == "table" and type(result.then_) == "function" then
            -- If the handler returns a Promise, chain it
            result:then_(resolve):catch_(reject)
          else
            resolve(result)
          end
        else
          reject(result)
        end
      end,
      on_rejected = function(err)
        reject(err)
      end,
    })
    if self._state ~= "pending" then
      self:_call_handlers()
    end
  end)
end

--- Register rejection handler.
--- @param on_rejected function(err)  Called when promise is rejected
--- @return Promise
function Promise:catch_(on_rejected)
  return M.new(function(resolve, reject)
    table.insert(self._handlers, {
      on_fulfilled = function(value)
        resolve(value)
      end,
      on_rejected = function(err)
        local ok, result = pcall(on_rejected, err)
        if ok then
          resolve(result)  -- recovery: treat as resolved
        else
          reject(result)
        end
      end,
    })
    if self._state ~= "pending" then
      self:_call_handlers()
    end
  end)
end

--- Register a handler that runs regardless of fulfillment or rejection.
--- @param fn function()
--- @return Promise
function Promise:finally_(fn)
  return M.new(function(resolve, reject)
    self:then_(function(value)
      fn()
      resolve(value)
    end):catch_(function(err)
      fn()
      reject(err)
    end)
  end)
end

--- Resolve immediately with a value.
--- @param value any
--- @return Promise
function M.resolve(value)
  return M.new(function(resolve) resolve(value) end)
end

--- Reject immediately with an error.
--- @param err any
--- @return Promise
function M.reject(err)
  return M.new(nil, function(reject) reject(err) end)
end

--- Wait for all promises to settle.
--- Returns Promise that resolves with array of values.
--- @param promises Promise[]
--- @return Promise
function M.all(promises)
  return M.new(function(resolve, reject)
    if #promises == 0 then
      resolve({})
      return
    end
    local results = {}
    local remaining = #promises
    for i, p in ipairs(promises) do
      p:then_(function(value)
        results[i] = value
        remaining = remaining - 1
        if remaining == 0 then
          resolve(results)
        end
      end):catch_(function(err)
        reject(err)
      end)
    end
  end)
end

-- Internal: call pending handlers
function Promise:_call_handlers()
  if self._state == "pending" then return end
  local handlers = self._handlers
  self._handlers = {}  -- prevent re-entry
  for _, h in ipairs(handlers) do
    if self._state == "fulfilled" and h.on_fulfilled then
      h.on_fulfilled(self._value)
    elseif self._state == "rejected" and h.on_rejected then
      h.on_rejected(self._value)
    end
  end
end

return M
