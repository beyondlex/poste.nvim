---
name: poste-neovim-performance
description: >
  Use this skill when auditing, planning, or implementing performance
  improvements for Poste's Neovim plugin code. It guides agents to review Lua
  modules as senior Neovim plugin developers, preserve user interaction and UI
  behavior, identify hot paths, propose measured optimizations, and produce a
  staged refactoring plan for high-performance Neovim code.
allowed-tools:
  - Read
  - Write
  - Edit
  - Bash
  - AskUserQuestion
metadata:
  trigger: performance|slow|latency|stall|optimize|hot path|profiling|vim.fn.system|jobstart
---

# Poste Neovim Performance Skill

## When To Use

Load this skill for tasks involving:

- Performance review of `lua/poste/` or `tests/*.lua`
- Neovim-side optimization for HTTP, SQL, or Redis request execution
- SQL completion, dataset buffers, highlights, navigation, search, or db browser performance
- UI stalls, synchronous blocking, repeated scans, repeated `require()`, or repeated rendering
- Refactoring plans that preserve the current user interaction model and UI
- Performance rules for other agents working on Poste's Neovim plugin

## Core Goal

The default target is: same interaction, lower latency, less blocking. Do not turn a performance task into a UI or workflow redesign.

Priorities:

1. Preserve existing keymaps, commands, buffer names, filetypes, window behavior, winbar text, and result display semantics.
2. Identify hot paths before editing code. If evidence is indirect, label the issue as suspected.
3. First reduce synchronous blocking, repeated whole-buffer scans, repeated rendering, external process frequency, and high-frequency autocmd cost.
4. Make small staged changes with clear rollback paths.
5. Preserve HTTP/SQL isolation. HTTP changes must not affect SQL behavior, and SQL changes must not affect HTTP behavior.

## Poste Neovim Boundaries

Important files:

- `lua/poste/init.lua`: HTTP/Redis entry point and request orchestration
- `lua/poste/buffer.lua`: HTTP result buffer
- `lua/poste/completion.lua`: HTTP completion
- `lua/poste/sql/init.lua`: SQL request entry point
- `lua/poste/sql/completion.lua`: SQL completion orchestration
- `lua/poste/sql/completion_data.lua`: SQL completion async data, cache, fallback sources
- `lua/poste/sql/buffer.lua`: SQL dataset buffer creation, tabs, rendering
- `lua/poste/sql/buffer_nav.lua`: dataset cell navigation, preview, yank, sort
- `lua/poste/sql/buffer_search.lua`: dataset search and filtering
- `lua/poste/sql/highlights.lua`: dataset highlights and namespace management
- `lua/poste/state.lua`: shared state; SQL-specific state must stay under the `.sql` namespace

Isolation rules:

- SQL-only logic belongs in `lua/poste/sql/`.
- HTTP/Redis-only logic belongs in non-SQL `lua/poste/` modules.
- Shared state should extend the existing `state.lua` structure.
- Do not introduce hidden cross-protocol coupling for performance work.

## Audit Workflow

### 1. Establish The User Path

First identify the path being optimized:

- Insert-mode completion
- `poste run` request execution
- SQL result rendering
- dataset tab switching
- large result set navigation
- search, filter, or sort
- highlights or autocmds
- db browser open, refresh, or expand

Record trigger frequency:

- every keystroke
- every cursor move
- every completion source call
- every request completion
- every tab switch
- every buffer/window enter

High-frequency paths require stricter discipline: avoid I/O, external processes, whole-buffer scans, full JSON decode, and full highlight rebuilds.

### 2. Gather Evidence Quickly

Use local search to find likely risk points:

```bash
rg "vim.fn.system|jobstart|nvim_buf_get_lines|nvim_buf_set_lines|nvim_buf_add_highlight|nvim_buf_set_extmark|CursorMoved|TextChanged|CompleteChanged|vim.schedule|vim.defer_fn|json.decode|require\\(" lua/poste tests
```

Use Neovim profiling when behavior can be reproduced:

```vim
:profile start /tmp/poste-profile.log
:profile func *
:profile file */lua/poste/*
" reproduce the slow action
:profile stop
```

Temporary `vim.loop.hrtime()` instrumentation is acceptable while investigating. Remove temporary logs before the final patch unless they are behind an existing controlled debug flag such as `vim.g.poste_sql_debug`.

### 3. Classify The Issue

Classify each issue instead of only saying it is slow:

- Synchronous blocking: `vim.fn.system()`, file I/O, shell calls, synchronous JSON handling
- High-frequency whole-buffer scans: `nvim_buf_get_lines(0, -1, ...)` during completion or cursor movement
- Repeated computation: same buffer, offset, dialect, dataset, or search query computed repeatedly
- Repeated rendering: unchanged data still triggers `nvim_buf_set_lines()` or full highlight rebuilds
- Highlight amplification: per-cell highlights across large result sets or frequent namespace clearing
- Callback storms: async results are not deduplicated or guarded by generation tokens
- Excess scheduling: deeply nested or high-frequency `vim.schedule()` calls
- Hot-path `require()`: acceptable in keymap callbacks, but avoid inside tight loops
- String churn: `..` concatenation in large loops instead of table buffers plus `table.concat`
- Table churn: excessive short-lived tables in large result-set loops

### 4. Assess Behavior Risk

For each recommendation, state whether it changes:

- key mappings
- buffer contents and filetype
- cursor position, scroll, or `leftcol`
- winbar/status text
- completion item `label`, `kind`, `insertText`, or ordering
- diagnostic/debug output
- SQL/HTTP isolation boundaries

Default to no visible behavior change. If a visible change is necessary, label it separately and ask for user confirmation before implementing.

## Optimization Rules

### Neovim API

- Read only the needed buffer range in hot paths; avoid whole-buffer reads.
- For large buffer updates, set `modifiable=true`, call `nvim_buf_set_lines()` once, then restore `modifiable=false`.
- Wrap invalid window/buffer cases with `pcall`, but do not hide business logic failures.
- Use `nvim_set_option_value()` with explicit `buf` or `win` scope.
- Batch, deduplicate, or debounce high-frequency UI updates.

### Completion

- Completion callbacks must return quickly or complete asynchronously.
- Avoid spawning the Rust binary synchronously on every input. Cache by text, offset, and dialect, or move the path async.
- Short-circuit directive lines, simple prefixes, and obvious fallback cases in Lua before expensive paths.
- Completion cache keys must include every input that affects results: connection, database, schema, table, and dialect.
- Old async completion results must not overwrite newer requests.

### Dataset Buffer

- Large result sets should prefer pagination, visible-range work, lazy highlights, and cached column widths/cell ranges.
- Tab switching should reuse computed padded lines, metadata, header indexes, and search matches.
- Navigation should update only the current and previous cell highlights when possible.
- Sort, filter, and search must distinguish data changes from view changes to avoid unnecessary JSON decode.
- Header floats and winbars should update only when their content changes.

### Highlights

- Keep namespaces layered: cell, search, header, and syntax-like highlights should not clear each other.
- Use range-limited or lazy highlights for large buffers.
- Do not rebuild all extmarks/highlights on high-frequency cursor movement.
- Cache invalidation should be precise: clear only when separators, column widths, data, or theme-related state changes.

### Async And External Processes

- User-visible long operations should use `jobstart()` or a `vim.system()`-style async path.
- Async requests need a generation token or request id so stale responses cannot mutate current state.
- Limit stdout/stderr debug logging size; large responses should not be copied into logs unbounded.
- Shell command arguments must remain shell-escaped. Do not trade safety for speed.

### Caching

Every cache needs:

- Complete keys: include all inputs that affect the result.
- Clear lifetime: use `changedtick`, connection, database, query, page, sort, or filter as appropriate.
- Explicit invalidation: keep it near the cache or behind a named function.
- Bounded memory: store large dataset caches on tab/dataset state rather than in unlimited globals.

## Review Output Format

For performance reviews, respond in this order:

1. Findings: performance issues ordered by severity, with file and line references.
2. Evidence: profile result, code-path inference, reproduction, or suspected.
3. Behavior Risk: whether UI or interaction changes.
4. Recommendations: smallest viable fixes.
5. Refactor Plan: staged P0/P1/P2 plan.
6. Validation: Lua/Rust tests and manual paths to verify.

Finding template:

```text
P1 lua/poste/sql/completion.lua:42
The completion hot path reads the entire buffer and synchronously calls an external process; typing in large SQL files can stall.
Evidence: code-path inference; runs on every completion source call.
Fix: cache block bounds and text by bufnr+changedtick; make context detect async or add a finer cache key.
Behavior risk: completion item semantics should stay unchanged; verify stale async results cannot overwrite newer requests.
```

Refactor plan template:

```text
P0: Low-risk containment
- Add caches, deduplication, range reads, or render guards.
- Preserve user interaction and UI exactly.

P1: Structure hot paths
- Extract high-frequency logic into testable pure functions.
- Introduce request ids, changedtick-aware cache keys, and explicit invalidation.
- Add focused Lua tests.

P2: Deeper architecture optimization
- Move synchronous bottlenecks to async paths.
- Add lazy rendering or virtualization for large datasets.
- Define cross-module performance contracts.
```

## Code Change Requirements

- Read the existing module and tests first; follow local style.
- Keep changes close to the hot path; avoid unrelated renames or broad rewrites.
- Change one performance assumption at a time so regressions are easy to isolate.
- Every new cache must include invalidation.
- Every new async path must handle cancellation/stale responses and errors.
- Preserve existing debug flag semantics such as `vim.g.poste_sql_debug`.
- Do not add new external Lua dependencies unless the user explicitly agrees.

## Validation Checklist

Consider at least:

```bash
tests/run.sh
cargo test
```

Choose manual checks based on the touched area:

- SQL completion: connection directive, database directive, table names, columns, aliases, schemas, dot columns
- HTTP completion: variables, scripts, request chaining
- SQL dataset: large result rendering, tab switching, horizontal scroll, cell navigation, search, sort, filter
- Request execution: success, stderr, JSON decode failure, pre-script, assertions
- UI state: cursor, `leftcol`, winbar, header float, filetype, `modifiable`

If tests are not run, state the unverified areas and residual risk.

## Do Not

- Do not use "seems faster" as evidence.
- Do not change default keymaps, buffer layout, completion semantics, or UI text for a performance-only task.
- Do not put SQL optimization in HTTP modules or HTTP state in SQL modules.
- Do not add more logs, notifications, synchronous shell calls, or whole-buffer scans to hot paths.
- Do not hide errors in ways that produce silent failure.
- Do not introduce unbounded global caches.
