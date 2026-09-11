-- Specs for the poste.select fallback float picker (the path taken when
-- snacks.nvim is not installed).
--
-- The specs invoke the buffer-local keymap callbacks directly via
-- maparg().callback instead of feeding keys: headless mode gives startinsert!
-- a phantom pending-insert state that swallows the first <Esc>, making
-- feedkeys-driven cancellation order-dependent. Calling the callbacks is
-- deterministic and pins the same wiring.

describe("poste.select", function()
  local select = require("poste.select")

  -- The search prompt prefix ("▸ "): line 1 of the float must start with it
  -- and insertions must land right after it.
  local PREFIX = "\239\134\133 "

  local function find_float_buf()
    for _, buf in ipairs(vim.api.nvim_list_bufs()) do
      if
        vim.api.nvim_buf_is_valid(buf)
        and vim.api.nvim_get_option_value("filetype", { buf = buf }) == "PosteSelect"
      then
        return buf
      end
    end
  end

  local function open_picker(items)
    local picked = { key = "<pending>" }
    select.select(items, "Pick", function(key)
      picked.key = key
    end)
    local buf = find_float_buf()
    assert(buf, "fallback float did not open")
    local win = vim.fn.bufwinid(buf)
    assert.are.not_equal(-1, win)
    return buf, win, picked
  end

  -- Fetch a buffer-local normal-mode keymap's Lua callback. Looks the map up
  -- on the picker's buffer so a stale map from an earlier picker can't leak in.
  local function nmap_cb(buf, key)
    vim.api.nvim_set_current_win(vim.fn.bufwinid(buf))
    local map = vim.fn.maparg(key, "n", false, true)
    if type(map) == "table" and map.buffer == 1 and type(map.callback) == "function" then
      return map.callback
    end
    error("no buffer-local normal-mode map for " .. key)
  end

  local function close_picker(buf)
    local q = nmap_cb(buf, "q")
    q()
    vim.wait(200, function() return false end)
    local leftover = find_float_buf()
    if leftover then
      -- resolve's nvim_win_close is a no-op when the float is the only
      -- window (E444, headless), so bufhidden=wipe never fires there
      vim.api.nvim_buf_delete(leftover, { force = true })
    end
  end

  it("opens the float with the cursor on the search line and cancels via q", function()
    local buf, win, picked = open_picker({ "alpha", "beta" })
    local lines = vim.api.nvim_buf_get_lines(buf, 0, -1, false)
    assert.equals(PREFIX, lines[1]:sub(1, #PREFIX))
    assert.equals("▶ alpha", lines[2])
    assert.equals("  beta", lines[3])
    -- nvim_win_set_cursor clamps the column to len-1, so the exact column
    -- after focusing the 4-byte prefix is 3 headless; startinsert! then
    -- appends past the prefix. Pinning the line is the stable invariant.
    assert.equals(1, vim.api.nvim_win_get_cursor(win)[1])
    close_picker(buf)
    assert.is_nil(picked.key)
  end)

  it("i returns a drifted cursor to the search line", function()
    local buf, win = open_picker({ "alpha", "beta", "gamma" })
    -- j/k move the selection highlight, not the cursor, but the cursor can
    -- still drift off the search line (mouse scroll, Ctrl-F …). i must
    -- refocus the search input instead of typing into the item list.
    vim.api.nvim_win_set_cursor(win, { 3, 0 })
    nmap_cb(buf, "i")()
    assert.equals(1, vim.api.nvim_win_get_cursor(win)[1])
    close_picker(buf)
  end)

  it("an empty item list resolves nil without opening a float", function()
    local picked = { key = "<pending>" }
    select.select({}, "Pick", function(key)
      picked.key = key
    end)
    vim.wait(200, function() return false end)
    assert.is_nil(picked.key)
    assert.is_nil(find_float_buf())
  end)
end)
