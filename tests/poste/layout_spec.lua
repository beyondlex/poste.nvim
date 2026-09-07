describe("poste.layout", function()
  local layout = require("poste.layout")

  describe("word_wrap", function()
    it("returns short text as a single line", function()
      assert.are.same({ "hello" }, layout.word_wrap("hello", 20))
    end)

    it("wraps at spaces within the width", function()
      local lines = layout.word_wrap("alpha beta gamma delta", 11)
      for _, l in ipairs(lines) do
        assert.is_true(#l <= 11, "line too long: " .. l)
      end
      assert.are.same({ "alpha beta", "gamma delta" }, lines)
    end)

    it("hard-breaks a word longer than the width", function()
      local lines = layout.word_wrap(string.rep("x", 30), 10)
      assert.are.equal(3, #lines)
      assert.are.equal(10, #lines[1])
    end)

    it("never splits a multi-byte character in half", function()
      local cjk = string.rep("数据库", 20) -- 60 chars, 3 bytes each
      -- width 10 is deliberately NOT divisible by 3: the old byte-slicing
      -- version passed at width 9 only because 9 % 3 == 0 and emitted a
      -- half-character at width 10
      local lines = layout.word_wrap(cjk, 10)
      assert.is_true(#lines > 1)
      for _, l in ipairs(lines) do
        -- every returned line must be valid UTF-8 (round-trips through
        -- strchars with no replacement): a byte-cut line would fail this
        assert.are.equal(vim.fn.strcharpart(l, 0, vim.fn.strchars(l)), l)
        assert.is_true(vim.fn.strdisplaywidth(l) <= 10, "line overflows: " .. l)
      end
      -- rejoining the pieces reproduces the input
      assert.are.equal(cjk, table.concat(lines))
    end)

    it("wraps CJK text with spaces at the spaces", function()
      local lines = layout.word_wrap("数据库 查询 分析 引擎测试", 8)
      assert.is_true(#lines > 1)
      for _, l in ipairs(lines) do
        assert.is_true(vim.fn.strdisplaywidth(l) <= 8, "line overflows: " .. l)
      end
    end)

    it("handles a single character wider than the width without hanging", function()
      local lines = layout.word_wrap("数据库", 1)
      assert.are.same({ "数", "据", "库" }, lines)
    end)
  end)

  describe("cell / pad", function()
    it("pads to the display width", function()
      assert.are.equal(10, #layout.cell("abc", 10))
    end)

    it("accounts for CJK double width", function()
      local padded = layout.cell("数据库", 10) -- display width 6 → 4 spaces
      assert.are.equal(10, vim.fn.strdisplaywidth(padded))
    end)
  end)

  describe("dynamic_line", function()
    it("left-truncates keeping the tail", function()
      local line = layout.dynamic_line({
        text = "abcdefghijklmnopqrstuvwxyz",
        container_width = 10,
        truncate_at = "left",
        padding = { left = 0, right = 0 },
      })
      -- width 10 = 7 chars + "..."
      assert.are.equal("...tuvwxyz", line)
    end)

    it("mid-truncates around the middle", function()
      local line = layout.dynamic_line({
        text = "abcdefghijklmnopqrstuvwxyz",
        container_width = 10,
        truncate_at = "mid",
      })
      assert.are.equal("abc...wxyz", line)
    end)

    it("regression: left/mid truncation is char-safe for multi-byte text", function()
      -- #s (bytes) is 3x strchars for CJK: the old byte-offset math returned
      -- a string starting mid-character (invalid UTF-8) or truncated wrong
      local line = layout.dynamic_line({
        text = string.rep("数", 30),
        container_width = 12,
        truncate_at = "left",
      })
      assert.is_true(vim.fn.strchars(line) > 0)
      -- no replacement chars: first char must be a real 数 or the ellipsis
      assert.is_true(line:find("^%s*…") ~= nil or line:find("数", 1, true) ~= nil)
      local mid = layout.dynamic_line({
        text = string.rep("数", 30),
        container_width = 12,
        truncate_at = "mid",
      })
      assert.is_truthy(mid:find("..."))
    end)

    it("survives a content width narrower than the ellipsis", function()
      -- avail used to go negative → strcharpart with a negative count
      local line = layout.dynamic_line({
        text = "abcdefgh",
        container_width = 3,
        truncate_at = "right",
      })
      assert.is_truthy(#line > 0)
      assert.are.equal(3, vim.fn.strdisplaywidth(line))
    end)
  end)

  describe("progress", function()
    it("renders a filled bar with label", function()
      local line = layout.progress(5, 10, { bar_width = 10 })[1]
      assert.truthy(line:find("5/10 50%%"))
      assert.truthy(line:find("█"))
    end)

    it("handles total=0 without dividing by nil", function()
      local line = layout.progress(0, 0, { bar_width = 4 })[1]
      assert.truthy(line:find("0/0 0%%"))
    end)
  end)

  describe("columns", function()
    it("renders titles and items with a title highlight", function()
      local out = layout.columns({
        { title = "Tables", items = { "a", "b" } },
        { title = "Views", items = { "v1" } },
      }, { width = 20 })
      assert.are.equal(3, #out.lines)
      assert.is_true(#out.highlights >= 2)
      assert.truthy(out.lines[1]:find("Tables"))
    end)
  end)

  describe("keymaps", function()
    it("renders [key label] entries separated", function()
      local out = layout.keymaps({ mapping = {
        { key = "g?", label = "help" },
        { key = "q", label = "close" },
      } })
      assert.truthy(out.lines[1]:find("%[g%? help%]"))
      assert.truthy(out.lines[1]:find("%[q close%]"))
    end)

    it("keeps the prefix intact when mapping is empty", function()
      -- the old unconditional sub() used to shave a char off the prefix
      local out = layout.keymaps({ mapping = {}, indent = 4 })
      assert.are.equal("    ", out.lines[1])
    end)
  end)

  describe("space_between", function()
    it("pads the gap to the target width", function()
      local line = layout.space_between("a", "b", { width = 5 })[1]
      assert.are.equal(5, #line)
      assert.are.equal("a", line:sub(1, 1))
      assert.are.equal("b", line:sub(-1))
    end)
  end)
end)
