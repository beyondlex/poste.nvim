--- Shared statusline context composition for the poste plugin family.
---
--- Sibling plugins (poste-db, poste-redis, ...) resolve the *window under the
--- cursor's* context — connection (+ scope) text plus a per-connection
--- highlight group — and register that as a provider here. ONE neutral
--- mini.statusline `content.active` + `section_fileinfo` renders the resolved
--- context, so plugins stop fighting over the same global hooks: whoever
--- loads first no longer decides which context wins.
---
--- Resolution is order-independent: providers declare how specific their
--- match is (`buffer` > `window` > `global`), and per window the most
--- specific non-empty context wins. A SQL buffer's poste-db context
--- therefore beats a redis *global* fallback, while a redis result panel
--- (window-scoped) claims every context on that window.
---
--- The highlight survives even when some other layout owns `content.active`,
--- because `section_fileinfo` bakes `%#…#` markup into the context string
--- itself (`my-blog/blog` renders highlighted under any layout).
---
--- Provider contract
--- -----------------
--- spec = { name = string, resolve = fun(win) -> ctx|nil } with
--- ctx = { text = string, hl = string|nil, scope = "buffer"|"window"|"global" }.
---
--- - `text`/`hl` must be plain strings; the compose layer escapes `%` in
---   both, so provider code never touches statusline markup itself.
--- - `hl` groups belong to the provider and must be namespaced per plugin
---   (`Poste<Plugin>Ctx…`, e.g. `PosteDbSqlCtxprod`, `PosteRedisCtxlocal`);
---   the compose layer never defines highlight groups.
--- - `resolve` runs on every statusline redraw (twice: `content.active`
---   queries the hl, `section_fileinfo` queries the markup) and must be
---   cheap — memoize on the provider side (fingerprint caches etc.).
--- - A provider that errors or returns garbage (nil / non-string / empty
---   `text`) is skipped, so one broken sibling never blanks the family
---   statusline.
--- - Registering an existing `name` replaces that entry in place (plugin
---   reloads must not stack providers; the original slot — and therefore
---   tie-break order — is preserved). `unregister_provider(name)` removes it.
local M = {}

local SCOPE_RANK = { buffer = 3, window = 2, global = 1 }

--- Registered providers: { name = string, resolve = function(win) }.
local providers = {}

--- True once the mini.statusline hooks are installed (idempotent).
local installed = false

--- Register a window-context provider.
--- @param spec table { name = string,
---   resolve = (function(win) -> { text = string, hl = string|nil,
---                  scope = "buffer"|"window"|"global" } | nil) |
---   nil when the window does not belong to the provider (no context to show).
---   `scope` defaults to "window".
--- Registering an already-known name replaces the previous entry in place,
--- so plugin reloads don't stack providers (see header: Provider contract).
function M.register_provider(spec)
  if type(spec) ~= "table" or type(spec.name) ~= "string" or spec.name == ""
      or type(spec.resolve) ~= "function" then
    require("poste.error").warn(
      ("poste.statusline: ignoring invalid provider spec (name=%s)")
        :format(type(spec) == "table" and tostring(spec.name) or tostring(spec)))
    return
  end
  for i, p in ipairs(providers) do
    if p.name == spec.name then
      providers[i] = { name = spec.name, resolve = spec.resolve }
      return
    end
  end
  providers[#providers + 1] = {
    name = spec.name,
    resolve = spec.resolve,
  }
end

--- Remove a previously registered provider (plugin teardown / test setup).
--- @param name string
function M.unregister_provider(name)
  for i, p in ipairs(providers) do
    if p.name == name then
      table.remove(providers, i)
      return
    end
  end
end

--- Resolve the current window's context across all providers: the most
--- specific non-empty result wins (buffer > window > global; first
--- registration breaks ties).
--- @param win number|nil  default: current window
--- @return { text = string, hl = string|nil }|nil  nil when no provider
---   claims the window
function M.resolve(win)
  win = win or vim.api.nvim_get_current_win()
  if not vim.api.nvim_win_is_valid(win) then return nil end
  local best_rank, best = nil, nil
  for _, p in ipairs(providers) do
    local ok, r = pcall(p.resolve, win)
    -- type(r) == "table" + string text: malformed returns are skipped here so
    -- markup() below can safely gsub the text
    if ok and type(r) == "table" and type(r.text) == "string" and r.text ~= "" then
      local rank = SCOPE_RANK[r.scope] or 2
      if not best or rank > best_rank then
        best_rank = rank
        best = r
      end
    end
  end
  if not best then return nil end
  return { text = best.text, hl = best.hl }
end

--- Context segment for the current window with `%#…#` markup baked in, or
--- "" when no provider claims it.
--- @param win number|nil
--- @return string
local function markup(win)
  local ctx = M.resolve(win)
  if not ctx then return "" end
  -- `%` is the statusline escape character: a connection named "100%" baked
  -- in raw makes every statusline redraw fail with E539 ("Illegal
  -- character"). Doubling it renders a literal `%`.
  local text = ctx.text:gsub("%%", "%%%%")
  if type(ctx.hl) == "string" and ctx.hl ~= "" then
    local hl = ctx.hl:gsub("%%", "%%%%")
    return "%#" .. hl .. "# " .. text .. " "
  end
  return text
end

--- Install the neutral mini.statusline hooks (synchronously; setup() defers
--- this so a lazily-loaded mini.nvim is honoured). Idempotent: the first
--- caller wires the hooks. The content.active layout mirrors the default
--- mini active layout so behaviour is unchanged when no provider matches;
--- the fileinfo group carries the context highlight and the installed
--- section_fileinfo string already embeds the context markup.
local function install()
  if installed then return end
  local ok_statusline, statusline = pcall(require, "mini.statusline")
  if not ok_statusline then return end
  installed = true

  local orig_fileinfo = statusline.section_fileinfo
  statusline.section_fileinfo = function(...)
    local m = markup()
    if m ~= "" then return m end
    return orig_fileinfo(...)
  end

  statusline.config.content.active = function()
    local ctx = M.resolve()
    local ctx_hl = ctx and ctx.hl or nil

    local mode, mode_hl = statusline.section_mode({ trunc_width = 120 })
    local git = statusline.section_git({ trunc_width = 40 })
    local diff = statusline.section_diff({ trunc_width = 75 })
    local diagnostics = statusline.section_diagnostics({ trunc_width = 75 })
    local lsp = statusline.section_lsp({ trunc_width = 75 })
    local filename = statusline.section_filename({ trunc_width = 140 })
    local fileinfo = statusline.section_fileinfo({ trunc_width = 120 })
    local location = statusline.section_location({ trunc_width = 75 })
    local search = statusline.section_searchcount({ trunc_width = 75 })

    return statusline.combine_groups({
      { hl = mode_hl,                 strings = { mode } },
      { hl = "MiniStatuslineDevinfo", strings = { git, diff, diagnostics, lsp } },
      "%<",
      { hl = "MiniStatuslineFilename", strings = { filename } },
      "%=",
      { hl = ctx_hl or "MiniStatuslineFileinfo", strings = { fileinfo } },
      { hl = mode_hl,                 strings = { location } },
      { hl = "MiniStatuslineFileinfo", strings = { search } },
    })
  end
end

--- Install the neutral mini.statusline hooks, deferred so a lazily-loaded
--- mini.nvim is honoured. Idempotent: every plugin's statusline.setup()
--- calls this — the first caller wires the hooks, the rest only register
--- providers.
function M.setup()
  vim.schedule(install)
end

M._test = {
  reset = function()
    providers = {}
    installed = false
  end,
  install = install,
  SCOPE_RANK = SCOPE_RANK,
}

return M