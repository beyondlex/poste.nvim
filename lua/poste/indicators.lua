--- Request line status indicators: the status icon (spinner / ✓ / ✘)
--- renders in the SIGN COLUMN so it never overlaps request text; latency /
--- assertion summaries render as end-of-line virtual text.
local uv = vim.uv or vim.loop

local C = require("poste.constants")

local M = {}

local indicator_ns = vim.api.nvim_create_namespace(C.INDICATOR_NS_NAME)
local spinner_timer = nil

local spinner_frames = C.SPINNER_FRAMES
local sign_group = C.SIGN_GROUP_NAME .. "_indicator"

-- Track which lines have active spinner signs so they can be replaced/cleared.
-- buf -> { line_0 = current_frame } (current_frame starts at 1)
local spinner_signs = {}  -- buf -> { line_0 = frame }

local function define_signs()
  for i, frame in ipairs(spinner_frames) do
    pcall(vim.fn.sign_define, "PosteSpin" .. i, { text = frame .. " ", texthl = "PosteSpinner" })
  end
  pcall(vim.fn.sign_define, "PosteIndicatorSuccess", { text = "✓ ", texthl = "PosteSuccess" })
  pcall(vim.fn.sign_define, "PosteIndicatorError",   { text = "✘ ", texthl = "PosteError" })
end
define_signs()

--- Unplace every indicator sign on the target line (0-based). sign_unplace's
--- dict only honors `buffer` and `id` — `lnum` is silently ignored, so passing
--- it would delete ALL signs in the group on the buffer, wiping sibling
--- indicators (e.g. other statements in a multi-statement run).
local function unplace_sign_at_line(buf, line_0)
  local lnum = line_0 + 1
  local placed = vim.fn.sign_getplaced(buf, { group = sign_group })
  for _, s in ipairs(placed[1].signs) do
    if s.lnum == lnum then
      vim.fn.sign_unplace(sign_group, { buffer = buf, id = s.id })
    end
  end
end

local function place_sign(buf, line_0, name)
  unplace_sign_at_line(buf, line_0)
  vim.fn.sign_place(0, sign_group, name, buf, { lnum = line_0 + 1 })
end

local function unplace_sign(buf, line_0)
  unplace_sign_at_line(buf, line_0)
end

local function any_active_spinners()
  for _, lines in pairs(spinner_signs) do
    if next(lines) then return true end
  end
  return false
end

local function stop_timer()
  if spinner_timer then
    spinner_timer:stop()
    spinner_timer:close()
    spinner_timer = nil
  end
end

-- One shared timer advances the frame on EVERY active spinner line, so a
-- multi-statement run can spin several signs at once (a per-line timer would
-- be stopped by the next set_indicator call and freeze earlier spinners).
local function start_timer()
  if spinner_timer then return end
  spinner_timer = uv.new_timer()
  spinner_timer:start(C.SPINNER_INTERVAL_MS, C.SPINNER_INTERVAL_MS, vim.schedule_wrap(function()
    for buf, lines in pairs(spinner_signs) do
      if not vim.api.nvim_buf_is_valid(buf) then
        spinner_signs[buf] = nil
      else
        for line_0, frame in pairs(lines) do
          frame = (frame % #spinner_frames) + 1
          lines[line_0] = frame
          place_sign(buf, line_0, "PosteSpin" .. frame)
        end
      end
    end
    if not any_active_spinners() then stop_timer() end
  end))
end

--- Clear all indicators for a buffer.
function M.clear_all(buf)
  if not buf or not vim.api.nvim_buf_is_valid(buf) then return end
  vim.api.nvim_buf_clear_namespace(buf, indicator_ns, 0, -1)
  -- Unplace every sign in the indicator group on this buffer (success/error
  -- signs too, which are never tracked in spinner_signs).
  local placed = vim.fn.sign_getplaced(buf, { group = sign_group })
  for _, s in ipairs(placed[1].signs) do
    vim.fn.sign_unplace(sign_group, { buffer = buf, id = s.id })
  end
  spinner_signs[buf] = nil
  if not any_active_spinners() then stop_timer() end
end

--- Clear indicators for all lines except the current one.
function M.clear_other_requests(buf, _line_0)
  if not buf or not vim.api.nvim_buf_is_valid(buf) then return end
  vim.api.nvim_buf_clear_namespace(buf, indicator_ns, 0, -1)
end

--- Format latency for display.
local function format_latency(latency_ms)
  if latency_ms >= 1000 then
    return string.format("%.2f s", latency_ms / 1000)
  end
  return string.format("%.2f ms", latency_ms)
end

--- Build assertion summary text and highlight group.
local function build_assertion_text(assertion_results)
  if not assertion_results or not assertion_results.total or assertion_results.total == 0 then
    return nil
  end
  if assertion_results.failed and assertion_results.failed > 0 then
    return {
      text = string.format("✘ %d/%d tests", assertion_results.failed, assertion_results.total),
      hl = "PosteError",
    }
  end
  return {
    text = string.format("✓ %d/%d tests", assertion_results.passed, assertion_results.total),
    hl = "PosteSuccess",
  }
end

--- Build end-of-line virt_text from latency and assertion results (the
--- status icon itself lives in the sign column).
local function build_virt_text(latency_ms, assertion_results)
  local virt_text = {}
  if latency_ms and latency_ms > 0 then
    table.insert(virt_text, { " " .. format_latency(latency_ms), "PosteLatency" })
  end
  local assert_item = build_assertion_text(assertion_results)
  if assert_item then
    table.insert(virt_text, { " " .. assert_item.text, assert_item.hl })
  end
  return virt_text
end

--- Place or update the indicator.
--- status: "running" | "success" | "error"
function M.set_indicator(buf, line_0, status, latency_ms, assertion_results)
  if not buf or not vim.api.nvim_buf_is_valid(buf) then return end
  if not line_0 then return end

  if status == "running" then
    if not spinner_signs[buf] then spinner_signs[buf] = {} end
    spinner_signs[buf][line_0] = 1
    place_sign(buf, line_0, "PosteSpin1")
    start_timer()

  elseif status == "success" or status == "error" then
    unplace_sign(buf, line_0)
    if spinner_signs[buf] then spinner_signs[buf][line_0] = nil end
    local icon = status == "success" and "PosteIndicatorSuccess" or "PosteIndicatorError"
    place_sign(buf, line_0, icon)
    local virt = build_virt_text(latency_ms, assertion_results)
    if #virt > 0 then
      vim.api.nvim_buf_set_extmark(buf, indicator_ns, line_0, 0, {
        virt_text = virt,
        virt_text_pos = "eol",
        hl_mode = "combine",
      })
    end
  end
end

return M
