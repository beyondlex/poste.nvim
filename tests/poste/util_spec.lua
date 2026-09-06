describe("poste.util", function()
  local util = require("poste.util")

  describe("clean_nil", function()
    it("removes vim.NIL values in place", function()
      local t = { a = vim.NIL, b = 1, nested = { c = vim.NIL, d = "x" } }
      util.clean_nil(t)
      assert.is_nil(t.a)
      assert.are.equal(1, t.b)
      assert.is_nil(t.nested.c)
      assert.are.equal("x", t.nested.d)
    end)

    it("passes through non-tables", function()
      assert.is_nil(util.clean_nil(nil))
      assert.are.equal("s", util.clean_nil("s"))
    end)
  end)

  describe("ensure_job_data", function()
    it("strips trailing empty strings", function()
      assert.are.same({ "a", "b" }, util.ensure_job_data({ "a", "b", "" }))
      assert.are.same({}, util.ensure_job_data({ "" }))
      assert.are.same({}, util.ensure_job_data(nil))
    end)
  end)

  describe("find_file_upwards", function()
    local tmp

    before_each(function() tmp = vim.fn.tempname() end)
    after_each(function() vim.fn.delete(tmp, "rf") end)

    it("finds the file in the start dir", function()
      vim.fn.mkdir(tmp, "p")
      vim.fn.writefile({ "x" }, tmp .. "/marker.json")
      assert.are.equal(tmp .. "/marker.json", util.find_file_upwards("marker.json", tmp))
    end)

    it("walks up to an ancestor", function()
      vim.fn.mkdir(tmp .. "/a/b", "p")
      vim.fn.writefile({ "x" }, tmp .. "/marker.json")
      assert.are.equal(tmp .. "/marker.json", util.find_file_upwards("marker.json", tmp .. "/a/b"))
    end)

    it("returns nil when nothing matches", function()
      vim.fn.mkdir(tmp, "p")
      assert.is_nil(util.find_file_upwards("definitely-missing-xyz.json", tmp))
    end)
  end)
end)
