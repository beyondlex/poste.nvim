describe("poste.async.promise", function()
  local P = require("poste.async.promise")

  it("resolves and chains through then_", function()
    local got
    P.new(function(resolve)
      resolve(21)
    end):then_(function(v) return v * 2 end):then_(function(v) got = v end)
    assert.are.equal(42, got)
  end)

  it("chains a promise returned from then_", function()
    local got
    P.resolve(1):then_(function(v)
      return P.new(function(resolve)
        vim.schedule(function() resolve(v + 1) end)
      end)
    end):then_(function(v) got = v end)
    vim.wait(1000, function() return got ~= nil end)
    assert.are.equal(2, got)
  end)

  it("rejects on executor error and routes through catch_", function()
    local err
    P.new(function() error("boom") end):catch_(function(e) err = e end)
    assert.truthy(tostring(err):find("boom"))
  end)

  it("recovery in catch_ resolves the chain", function()
    local got
    P.reject("no"):catch_(function() return "recovered" end):then_(function(v) got = v end)
    assert.are.equal("recovered", got)
  end)

  it("finally_ runs on both paths", function()
    local runs = 0
    P.resolve(1):finally_(function() runs = runs + 1 end)
    P.reject("e"):catch_(function() end):finally_(function() runs = runs + 10 end)
    assert.are.equal(11, runs)
  end)

  it("all() resolves with every value in order", function()
    local got
    P.all({ P.resolve("a"), P.resolve("b"), P.resolve("c") }):then_(function(v) got = v end)
    assert.are.same({ "a", "b", "c" }, got)
  end)

  it("all() with an empty list resolves immediately", function()
    local got
    P.all({}):then_(function(v) got = v end)
    assert.are.same({}, got)
  end)

  it("handlers registered after settlement still fire exactly once", function()
    local n = 0
    local p = P.resolve("x")
    p:then_(function() n = n + 1 end)
    p:then_(function() n = n + 1 end)
    assert.are.equal(2, n)
  end)

  it("ignores a second resolve/reject", function()
    local got
    local p = P.new(function(resolve, reject)
      resolve("first")
      resolve("second")
      reject("third")
    end)
    p:then_(function(v) got = v end)
    assert.are.equal("first", got)
  end)
end)
