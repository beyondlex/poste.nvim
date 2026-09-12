--- Shared statusline compose-layer spec: provider resolution (scope-based,
--- order-independent) and the neutral mini.statusline wiring.
local shared = require("poste.statusline")

--- Fake mini.statusline with just enough surface for the installed hooks.
local function fake_mini()
  local fake = {
    section_fileinfo = function() return "[orig]" end,
    config = { content = {} },
    combine_groups = function(groups)
      local out = {}
      for _, g in ipairs(groups) do
        if type(g) == "string" then
          out[#out + 1] = g
        elseif g.strings then
          if g.hl then out[#out + 1] = "%#" .. g.hl .. "#" end
          for _, s in ipairs(g.strings) do out[#out + 1] = tostring(s) end
        end
      end
      return table.concat(out)
    end,
    section_mode = function() return "M", "ModeHl" end,
    section_git = function() return "G" end,
    section_diff = function() return "D" end,
    section_diagnostics = function() return "DG" end,
    section_lsp = function() return "L" end,
    section_filename = function() return "F" end,
    section_location = function() return "LOC" end,
    section_searchcount = function() return "S" end,
  }
  package.loaded["mini.statusline"] = fake
  return fake
end

describe("poste.statusline", function()
  before_each(function()
    shared._test.reset()
  end)

  after_each(function()
    shared._test.reset()
    package.loaded["mini.statusline"] = nil
  end)

  describe("resolve", function()
    local function make_ctx_buf(ctx)
      local buf = vim.api.nvim_create_buf(false, true)
      vim.b[buf].poste_db_context = ctx
      vim.api.nvim_set_current_buf(buf)
      return buf
    end

    it("returns nil when no provider claims the window", function()
      assert.is_nil(shared.resolve())
    end)

    it("uses the first non-empty provider", function()
      shared.register_provider({ name = "a", resolve = function() return nil end })
      shared.register_provider({
        name = "b", resolve = function() return { text = "b/x", hl = "Hlb" } end,
      })
      local r = shared.resolve()
      assert.equals("b/x", r.text)
      assert.equals("Hlb", r.hl)
    end)

    it("buffer scope beats a sibling's global fallback", function()
      local db = {
        name = "db",
        resolve = function(win)
          local ctx = vim.b[vim.api.nvim_win_get_buf(win)].poste_db_context
          if not ctx then return nil end
          return { text = ctx, hl = "Ctx" .. ctx, scope = "buffer" }
        end,
      }
      local redis_global = {
        name = "redis",
        resolve = function() return { text = "g", hl = "Ctxg", scope = "global" } end,
      }
      shared.register_provider(db)
      shared.register_provider(redis_global)

      local buf = make_ctx_buf("prod/blog")
      assert.equals("prod/blog", shared.resolve().text)

      vim.api.nvim_buf_delete(buf, { force = true })
      local plain = vim.api.nvim_create_buf(false, true)
      vim.api.nvim_set_current_buf(plain)
      assert.equals("g", shared.resolve().text)
      vim.api.nvim_buf_delete(plain, { force = true })
    end)

    it("buffer scope beats a sibling's window context on the same window", function()
      local special = vim.api.nvim_create_buf(false, true)
      vim.api.nvim_set_current_buf(special)
      vim.b.poste_db_context = "db/blog"

      shared.register_provider({
        name = "redis",
        resolve = function(win)
          if vim.api.nvim_win_get_buf(win) == special then
            return { text = "local db3", hl = "Ctxlocal", scope = "window" }
          end
          return nil
        end,
      })
      shared.register_provider({
        name = "db",
        resolve = function(win)
          local ctx = vim.b[vim.api.nvim_win_get_buf(win)].poste_db_context
          if not ctx then return nil end
          return { text = ctx, hl = "Ctxdb", scope = "buffer" }
        end,
      })
      assert.equals("db/blog", shared.resolve().text)

      vim.api.nvim_buf_delete(special, { force = true })
      local plain = vim.api.nvim_create_buf(false, true)
      vim.api.nvim_set_current_buf(plain)
      assert.is_nil(shared.resolve())  -- redis window provider no longer matches
      vim.api.nvim_buf_delete(plain, { force = true })
    end)

    it("equal-scope ties follow registration order", function()
      local mk = {}
      for i = 1, 3 do
        mk[i] = {
          name = "p" .. i,
          resolve = function() return { text = "c" .. i, scope = "global" } end,
        }
      end
      assert.is_nil(shared.resolve())  -- no provider yet: resolve is nil-safe
      shared.register_provider(mk[1])
      shared.register_provider(mk[2])
      shared.register_provider(mk[3])
      local first = shared.resolve().text
      assert.equals("c1", first, "first registration wins the tie")

      shared._test.reset()
      shared.register_provider(mk[3])
      shared.register_provider(mk[2])
      shared.register_provider(mk[1])
      assert.equals("c3", shared.resolve().text,
        "reversed registration order flips the winner, deterministically")
    end)

    it("ignores providers that error", function()
      shared.register_provider({ name = "boom", resolve = function() error("nope") end })
      shared.register_provider({
        name = "ok", resolve = function() return { text = "ok/blog" } end,
      })
      assert.equals("ok/blog", shared.resolve().text)
    end)
  end)

  describe("mini.statusline wiring", function()
    it("installs content.active + section_fileinfo with the provider context", function()
      local ms = fake_mini()
      shared.register_provider({
        name = "db",
        resolve = function()
          return { text = "prod/blog", hl = "PosteDbSqlCtxprod", scope = "buffer" }
        end,
      })
      shared._test.install()

      local fileinfo = ms.section_fileinfo({})
      assert.match("PosteDbSqlCtxprod# prod/blog", fileinfo)

      local active = ms.config.content.active()
      assert.match("prod/blog", active)
      assert.match("MiniStatuslineDevinfo", active)  -- layout intact
      assert.match("PosteDbSqlCtxprod", active)
    end)

    it("bakes hl-less contexts as plain text", function()
      local ms = fake_mini()
      shared.register_provider({
        name = "plain",
        resolve = function() return { text = "anon/blog", scope = "buffer" } end,
      })
      shared._test.install()
      assert.match("anon/blog", ms.section_fileinfo({}))
    end)

    it("delegates to the original fileinfo when no provider matches", function()
      local ms = fake_mini()
      shared._test.install()
      assert.match("[orig]", ms.section_fileinfo({}))
    end)

    it("wires once: a second install does not re-capture", function()
      local ms = fake_mini()
      shared.register_provider({
        name = "db",
        resolve = function()
          return { text = "prod/blog", hl = "PosteDbSqlCtxprod", scope = "buffer" }
        end,
      })
      shared._test.install()
      local active1 = ms.config.content.active

      -- a provider registered after the first install must still be seen
      shared.register_provider({
        name = "redis",
        resolve = function()
          return { text = "local db3", hl = "PosteRedisCtxlocal", scope = "global" }
        end,
      })
      shared._test.install()
      assert.equals(active1, ms.config.content.active,
        "the hooks must not be re-installed / re-captured")
      assert.match("prod/blog", ms.section_fileinfo({}),
        "buffer scope beats the newly-registered global provider")
    end)
  end)
end)