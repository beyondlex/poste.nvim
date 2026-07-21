--- Event Bus for Poste.
---
--- A lightweight publish-subscribe mechanism to decouple state producers from
--- consumers.  Modules emit events instead of writing directly to global state;
--- consumers subscribe to events instead of reading global state directly.
---
--- Usage:
---   local event = require("poste.state.event")
---
---   -- Subscribe
---   local unsub = event.on("response:ready", function(data)
---     print("Got response:", data.status)
---   end)
---
---   -- Emit
---   event.emit("response:ready", { status = 200, body = "..." })
---
---   -- Unsubscribe
---   unsub()
---
--- All handlers are wrapped in pcall so that a failing handler does not
--- prevent others from running.

local M = { _handlers = {} }

--- Register an event handler.
--- @param event string  Event name (e.g. "response:ready")
--- @param handler function(data)  Called when event is emitted
--- @return function  Unsubscribe function
function M.on(event, handler)
  M._handlers[event] = M._handlers[event] or {}
  table.insert(M._handlers[event], handler)
  -- Return unsubscribe function
  return function()
    local handlers = M._handlers[event]
    if not handlers then return end
    for i, h in ipairs(handlers) do
      if h == handler then
        table.remove(handlers, i)
        return
      end
    end
  end
end

--- Register a one-shot event handler (fires at most once).
--- @param event string  Event name
--- @param handler function(data)
--- @return function  Unsubscribe function
function M.once(event, handler)
  local wrapper
  wrapper = function(data)
    handler(data)
    -- Unsubscribe after first invocation
    local handlers = M._handlers[event]
    if not handlers then return end
    for i, h in ipairs(handlers) do
      if h == wrapper then
        table.remove(handlers, i)
        return
      end
    end
  end
  return M.on(event, wrapper)
end

--- Emit an event, calling all registered handlers with the given data.
--- Each handler is wrapped in pcall to isolate failures.
--- @param event string  Event name
--- @param data table|nil  Data passed to handlers
function M.emit(event, data)
  local handlers = M._handlers[event]
  if not handlers then return end
  -- Iterate over a copy to allow handlers to unsubscribe themselves
  local copy = vim.deepcopy(handlers)
  for _, handler in ipairs(copy) do
    local ok, err = pcall(handler, data)
    if not ok then
      vim.schedule(function()
        vim.notify(
          string.format("[poste] event '%s' handler error: %s", event, tostring(err)),
          vim.log.levels.ERROR
        )
      end)
    end
  end
end

--- Remove all handlers for a given event.
--- @param event string|nil  Event name, or nil to clear all events
function M.clear(event)
  if event then
    M._handlers[event] = nil
  else
    M._handlers = {}
  end
end

--- Get the number of handlers registered for an event.
--- @param event string  Event name
--- @return number
function M.handler_count(event)
  return #(M._handlers[event] or {})
end

return M
