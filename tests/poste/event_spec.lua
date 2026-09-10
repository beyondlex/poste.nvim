local event = require("poste.state.event")

describe("poste.state.event", function()
  before_each(function() event.clear() end)
  after_each(function() event.clear() end)

  it("delivers emitted data to subscribed handlers", function()
    local got
    event.on("test:evt", function(data) got = data end)
    event.emit("test:evt", { n = 1 })
    assert.same({ n = 1 }, got)
  end)

  it("unsubscribes via the returned function", function()
    local calls = 0
    local unsub = event.on("test:evt", function() calls = calls + 1 end)
    event.emit("test:evt")
    unsub()
    event.emit("test:evt")
    assert.equals(1, calls)
  end)

  it("once() fires exactly once", function()
    local calls = 0
    event.once("test:evt", function() calls = calls + 1 end)
    event.emit("test:evt")
    event.emit("test:evt")
    assert.equals(1, calls)
  end)

  it("once() unsubscribes even when the handler fails", function()
    -- a failing one-shot used to stay subscribed: the error skipped the
    -- unsubscribe, so every later emit re-ran (and re-reported) it
    local calls = 0
    event.once("test:evt", function()
      calls = calls + 1
      error("boom")
    end)
    local notified = 0
    local ok_notify = vim.notify
    vim.notify = function() notified = notified + 1 end
    event.emit("test:evt")
    event.emit("test:evt")
    -- emit reports through vim.schedule; drain the loop before counting or
    -- the assertion races the scheduled notify (flaky pass/fail)
    vim.wait(100, function() return notified > 0 end)
    vim.notify = ok_notify
    assert.equals(1, calls, "failing once-handler must not be retried")
    assert.equals(1, notified, "emit reports the failure exactly once")
  end)

  it("a failing handler does not stop later handlers", function()
    local ran = false
    event.on("test:evt", function() error("first fails") end)
    event.on("test:evt", function() ran = true end)
    local ok_notify = vim.notify
    vim.notify = function() end
    event.emit("test:evt")
    vim.notify = ok_notify
    assert.is_true(ran)
  end)

  it("clear(event) removes only that event's handlers", function()
    local a, b = 0, 0
    event.on("test:evt", function() a = a + 1 end)
    event.on("other:evt", function() b = b + 1 end)
    event.clear("test:evt")
    event.emit("test:evt")
    event.emit("other:evt")
    assert.equals(0, a)
    assert.equals(1, b)
  end)
end)
