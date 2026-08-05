--- Request line detection and status indicators (virt_text only: status icon + latency/assertions).
local uv = vim.uv or vim.loop

local C = require("poste.constants")

local M = {}

local indicator_ns = vim.api.nvim_create_namespace(C.INDICATOR_NS_NAME)
local spinner_timer = nil
local spinner_gen = 0

local spinner_frames = C.SPINNER_FRAMES

-- Track which lines have active spinner extmarks so we can update them
local spinner_marks = {}  -- buf -> { line_0 -> extmark_id }

local function stop_timer()
  spinner_gen = spinner_gen + 1
  if spinner_timer then
    spinner_timer:stop()
    spinner_timer:close()
    spinner_timer = nil
  end
end

--- Clear all indicators for a buffer.
function M.clear_all(buf)
  if not buf or not vim.api.nvim_buf_is_valid(buf) then return end
  vim.api.nvim_buf_clear_namespace(buf, indicator_ns, 0, -1)
  if spinner_marks[buf] then
    spinner_marks[buf] = {}
  end
  stop_timer()
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

--- Build virt_text table from status icon, latency, and assertion results.
local function build_virt_text(status_icon, status_hl, latency_ms, assertion_results)
  local virt_text = {}
  if status_icon then
    table.insert(virt_text, { " " .. status_icon, status_hl })
  end
  if latency_ms and latency_ms > 0 then
    table.insert(virt_text, { " " .. format_latency(latency_ms), "PosteLatency" })
  end
  local assert_item = build_assertion_text(assertion_results)
  if assert_item then
    table.insert(virt_text, { " " .. assert_item.text, assert_item.hl })
  end
  return virt_text
end

--- Clear all virt_text indicators for a buffer.
local function clear_all_virt_text(buf)
  if not buf or not vim.api.nvim_buf_is_valid(buf) then return end
  vim.api.nvim_buf_clear_namespace(buf, indicator_ns, 0, -1)
  if spinner_marks[buf] then
    spinner_marks[buf] = {}
  end
end

--- Place or update indicator (virt_text only: status icon + latency/assertions).
--- status: "running" | "success" | "error"
function M.set_indicator(buf, line_0, status, latency_ms, assertion_results)
  if not buf or not vim.api.nvim_buf_is_valid(buf) then return end
  if not line_0 then return end

  stop_timer()
  spinner_gen = spinner_gen + 1
  local my_gen = spinner_gen

  if status == "running" then
    if not spinner_marks[buf] then spinner_marks[buf] = {} end

    local frame = 1
    -- Place initial spinner virt_text
    local spinner_virt = build_virt_text(spinner_frames[frame], "PosteSpinner", latency_ms, assertion_results)
    if #spinner_virt > 0 then
      local mark_id = vim.api.nvim_buf_set_extmark(buf, indicator_ns, line_0, 0, {
        virt_text = spinner_virt,
        virt_text_pos = "eol",
        hl_mode = "combine",
      })
      spinner_marks[buf][line_0] = mark_id
    end

    local function update_spinner()
      if my_gen ~= spinner_gen then return end
      if not vim.api.nvim_buf_is_valid(buf) then return end
      local mark_id = spinner_marks[buf] and spinner_marks[buf][line_0]
      if not mark_id then return end
      -- Delete old extmark and place a new one with updated spinner frame
      pcall(vim.api.nvim_buf_del_extmark, buf, indicator_ns, mark_id)
      frame = (frame % #spinner_frames) + 1
      local spinner_update_virt = build_virt_text(spinner_frames[frame], "PosteSpinner", latency_ms, assertion_results)
      if #spinner_update_virt > 0 then
        local new_id = vim.api.nvim_buf_set_extmark(buf, indicator_ns, line_0, 0, {
          virt_text = spinner_update_virt,
          virt_text_pos = "eol",
          hl_mode = "combine",
        })
        spinner_marks[buf][line_0] = new_id
      end
    end

    spinner_timer = uv.new_timer()
    spinner_timer:start(C.SPINNER_INTERVAL_MS, C.SPINNER_INTERVAL_MS, vim.schedule_wrap(update_spinner))

  elseif status == "success" then
    if spinner_marks[buf] and spinner_marks[buf][line_0] then
      pcall(vim.api.nvim_buf_del_extmark, buf, indicator_ns, spinner_marks[buf][line_0])
      spinner_marks[buf][line_0] = nil
    end
    local virt = build_virt_text("✓", "PosteSuccess", latency_ms, assertion_results)
    if #virt > 0 then
      vim.api.nvim_buf_set_extmark(buf, indicator_ns, line_0, 0, {
        virt_text = success_virt,
        virt_text_pos = "eol",
        hl_mode = "combine",
      })
    end

  elseif status == "error" then
    if spinner_marks[buf] and spinner_marks[buf][line_0] then
      pcall(vim.api.nvim_buf_del_extmark, buf, indicator_ns, spinner_marks[buf][line_0])
      spinner_marks[buf][line_0] = nil
    end
    local virt = build_virt_text("✘", "PosteError", latency_ms, assertion_results)
    if #virt > 0 then
      vim.api.nvim_buf_set_extmark(buf, indicator_ns, line_0, 0, {
        virt_text = error_virt,
        virt_text_pos = "eol",
        hl_mode = "combine",
      })
    end
  end
end

return M
