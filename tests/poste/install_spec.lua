-- Regression coverage for install.ensure() resolution + version sync.
--
-- The version sync once lived in a `local function plugin_tag` declared
-- *after* its only call site: the call compiled to a global lookup (nil at
-- runtime), so the scheduled check errored whenever it ran. Worse, the
-- default state.config.poste_binary IS the managed data path, so step 1
-- returned before the sync's step-4 home was ever reached and the feature
-- was dead for the default configuration. These specs pin both behaviors:
-- the sync must run whenever the resolved binary is the managed install,
-- and the resolution chain must not depend on a global.

describe("poste.install", function()
  local install

  local fake_popen
  local popen_calls
  local real_popen

  -- A fake io.popen handle: plugin_tag() reads "" (no tag at HEAD) and
  -- closes; a nil tag means no download is attempted.
  local function fake_handle()
    return { read = function() return "" end, close = function() end }
  end

  before_each(function()
    install = require("poste.install")
    real_popen = real_popen or io.popen
    popen_calls = 0
    fake_popen = function()
      popen_calls = popen_calls + 1
      return fake_handle()
    end
    io.popen = fake_popen -- luacheck: ignore 122 (test double for plugin_tag's git probe)
    vim.g.poste_binary = nil
  end)

  after_each(function()
    io.popen = real_popen -- luacheck: ignore 122 (restore the real handle)
    vim.g.poste_binary = nil
  end)

  -- Wait out ensure()'s vim.schedule and report whether the sync ran.
  local function wait_for_sync()
    return vim.wait(500, function() return popen_calls > 0 end, 10)
  end

  describe("ensure", function()
    it("returns the managed install and runs the version sync for it", function()
      -- state.config.poste_binary defaults to the managed data path; a
      -- readable binary there must come back AND schedule the sync (the
      -- old step-1 early return silently skipped it).
      local managed = vim.fn.stdpath("data") .. "/poste/bin/poste"
      vim.fn.mkdir(vim.fn.fnamemodify(managed, ":h"), "p")
      vim.fn.writefile({ "#!/bin/sh" }, managed)
      vim.fn.setfperm(managed, "rwxr-xr-x")

      assert.are.equal(vim.fn.fnamemodify(managed, ":p"), install.ensure())
      assert.is_true(wait_for_sync(), "version sync did not run for the managed install")
      -- The call must not go through a global (the late-local regression).
      assert.is_nil(rawget(_G, "plugin_tag"))
    end)

    it("returns the vim.g.poste_binary override without touching releases", function()
      local override = vim.fn.stdpath("data") .. "/poste-override-poste"
      vim.fn.writefile({ "#!/bin/sh" }, override)

      vim.g.poste_binary = override
      assert.are.equal(vim.fn.fnamemodify(override, ":p"), install.ensure())

      -- An explicit override is a dev/worktree build: never sync or download.
      assert.is_false(wait_for_sync(), "override must not trigger the version sync")
    end)
  end)
end)
