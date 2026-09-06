describe("poste.state", function()
  local state = require("poste.state")

  describe("get_keymap", function()
    before_each(function()
      state.config.keymaps = { sec = { hit = "<CR>", off = false } }
    end)
    after_each(function() state.config.keymaps = nil end)

    it("returns the configured key", function()
      assert.are.equal("<CR>", state.get_keymap("sec", "hit"))
    end)

    it("false disables (nil), missing falls back to the default", function()
      assert.is_nil(state.get_keymap("sec", "off", "dflt"))
      assert.are.equal("dflt", state.get_keymap("sec", "missing", "dflt"))
      assert.are.equal("dflt", state.get_keymap("nope", "nope", "dflt"))
    end)
  end)

  describe("format_key_string", function()
    it("maps named keys for display", function()
      assert.are.equal("Enter", state.format_key_string("<CR>"))
      assert.are.equal("Esc", state.format_key_string("<Esc>"))
    end)

    it("renders <leader> with the actual mapleader", function()
      local old = vim.g.mapleader
      vim.g.mapleader = ","
      assert.are.equal(",ff", state.format_key_string("<leader>ff"))
      vim.g.mapleader = old
    end)

    it("passes plain keys through", function()
      assert.are.equal("gx", state.format_key_string("gx"))
      assert.are.equal("", state.format_key_string(nil))
    end)
  end)

  describe("find_poste_binary", function()
    after_each(function() vim.g.poste_binary = nil end)

    it("prefers vim.g.poste_binary when it exists", function()
      local exe = vim.fn.exepath("ls")
      if exe == "" then return end
      vim.g.poste_binary = exe
      assert.are.equal(vim.fn.fnamemodify(exe, ":p"), state.find_poste_binary())
    end)

    it("falls past a nonexistent vim.g.poste_binary", function()
      vim.g.poste_binary = "/definitely/not/here/poste"
      -- whatever comes back, it must not be the bogus path
      local p = state.find_poste_binary()
      assert.are_not.equal("/definitely/not/here/poste", p)
    end)
  end)
end)
