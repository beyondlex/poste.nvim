---
name: poste-refactor
description: >
  Guide for safe, behavior-preserving refactoring of Poste code (Rust + Lua).
  Covers core principles, step-by-step workflow, test preservation, and common
  pitfalls to avoid when restructuring code without changing observable behavior.
allowed-tools:
  - Read
  - Write
  - Edit
  - Bash
  - AskUserQuestion
metadata:
  trigger: refactor|restructure|rename|extract|split module|deduplicate|remove dead code|upgrade dependency
---

# Poste Refactoring Skill

## When To Use

Load this skill when the task involves:

- Restructuring Rust modules in `crates/poste-core/`, `crates/poste-exec/`, or `crates/poste-cli/`
- Restructuring Lua modules in `lua/poste/` or `lua/poste/sql/`
- Renaming types, functions, variables, or files
- Extracting shared logic from duplicate code
- Simplifying control flow or reducing nesting
- Splitting large modules into smaller ones
- Removing dead code or unused dependencies
- Upgrading dependencies or migrating APIs
- Improving error handling patterns (e.g., replacing `.unwrap()` with proper `Result` propagation)
- Reorganizing file layout without adding features

Do NOT load this skill for:

- Adding new features or protocols
- Performance optimization (load `poste-neovim-performance` instead)
- SQL completion changes (load `sql-completion` instead)
- Bug fixes that change observable behavior

## Core Principle: Behavior Preservation

**The first rule of refactoring: don't change what the code does. The second rule: don't break the tests.**

### What "No Behavior Change" Means

| Acceptable | Not Acceptable |
|---|---|
| Renaming internal identifiers | Changing function signatures callers depend on (without updating all callers) |
| Extracting duplicate logic into shared helpers | Changing error messages, log output, or diagnostic text |
| Splitting a module into multiple files | Changing JSON response fields consumed by Lua |
| Replacing `unwrap()` with `?` + proper error types | Changing completion item semantics or ordering |
| Simplifying control flow (same outcome) | Changing buffer content, cursor behavior, or filetype assignment |
| Removing dead code | Removing code that tests exercise (even if you think it's dead) |
| Inlining trivial functions | Changing visible UI text, keymaps, or commands |
| Restructuring types (same external API) | Changing default values, timeouts, or config loading behavior |

### Safe vs Unsafe Interfaces

These interfaces are consumed across the Rust/Lua boundary — changing their shape WILL break things:

- `poste context detect` JSON output fields consumed by `completion.lua`
- `poste run` output format consumed by `lua/poste/buffer.lua` and `lua/poste/sql/buffer.lua`
- CLI exit codes and stderr format
- Environment variable names in `env.json` / `connections.json`
- Completion mode flags (`vim.g.poste_sql_legacy_completion`, `vim.g.poste_sql_debug`)
- Buffer/filetype naming conventions

Before changing any of these, verify the consuming side handles the new format.

## Bad Smell Identification

Before any refactoring, the agent must autonomously scan the target code for bad smells. Use the following systematic checks.

### Rust Bad Smells

| Smell | Detection Method | Severity |
|-------|-----------------|----------|
| Function too long (>50 lines) | `awk '/^fn /{f=$0} /^    }/{if(NR-l>50) print f}'` or visual inspection | Medium |
| Module too large (>500 lines) | `wc -l <file>` | Medium |
| Duplicate code blocks | Search for repeated patterns (same sequence of calls in multiple places) | High |
| Nested `if-else` chains (>3 levels) | Scan for deep nesting in control flow | Medium |
| `unwrap()` outside tests | `rg '\.unwrap\(\)' -- '*.rs' \| rg -v '#\[cfg\(test\)\]\|#\[test\]'` | High |
| `expect("")` without message | `rg '\.expect\(""\)' -- '*.rs'` | Low |
| Dead code (unused `pub fn`/`pub struct`) | `rg '^pub (fn|struct|enum|type|const) \w+'` + cross-ref each | Medium |
| `todo!()` / `unreachable!()` in non-stub code | `rg 'todo!\|unreachable!' -- '*.rs'` | Medium |
| Functions with 6+ parameters | Inspect for struct extraction opportunities | Low |
| Large `match` arms with duplicated logic | Scan for identical blocks across match arms | High |
| Overly broad `pub` visibility | Check if `pub` items are used only within the crate | Low |
| Mix of error handling styles | Search for a mix of `anyhow`, `thiserror`, and raw `Result` in the same module | Low |
| Manual string building instead of `format!` | `rg '"\.\.' -- '*.rs'` (approximate) | Low |
| Unused `use` imports | `cargo check` will flag these | Low |

### Lua Bad Smells

| Smell | Detection Method | Severity |
|-------|-----------------|----------|
| Module too large (>300 lines) | `wc -l <file>` | Medium |
| Function too long (>30 lines) | Visual inspection | Medium |
| Duplicate code blocks | Search for repeated patterns across functions | High |
| Deep nesting (>3 levels) | Scan `if`/`for`/`while` chains | Medium |
| Global variable pollution | `rg '^\w+\s*=' -- '*.lua' \| rg -v 'local\|M\.\|vim\.\|self'` (excluding expected globals) | High |
| Strings built with `..` in loops | `rg '\.\..*for\|for.*\.\.' -- '*.lua'` | Medium |
| Dead code (unused `M.*` exports) | Cross-ref each `M.fn` against `require` calls | Medium |
| Magic strings/numbers | Search for bare string/number literals used multiple times | Low |
| Mixed HTTP/SQL logic | `rg 'require.*poste/sql' -- lua/poste/` (should not exist in non-sql dir) | High |

### When To Scan

- **Opened task without a specific file target**: scan the entire crate or directory
- **Task names a specific area** (e.g., "refactor sql_executor"): scan that file first, then related modules
- **During code review**: scan only the changed lines plus the enclosing function/module

### Scan Output

For each smell found, record:

```
<file>:<line>  <severity>  <smell>  <brief evidence>
```

Example:

```
crates/poste-exec/src/sql_executor.rs:142  HIGH  unwrap() outside tests  conn.unwrap() in production path
lua/poste/sql/buffer.lua:89             HIGH  global variable  results = {} without local
crates/poste-core/src/parser.rs:310     MED  >50 line function  fn parse_block() is 73 lines
```

## Refactoring Workflow

### Phase 1: Discovery — Identify Bad Smells

Before touching any code, the agent scans the target area for bad smells using the tables above. Run the detection commands and collect findings into a structured list. Do not skip this phase even if the task specifies what to refactor — there may be higher-value targets nearby.

Output the complete smell list with file:line references and severity. If the scan finds no smells, report that explicitly and do not proceed to refactoring.

### Phase 2: Report & Plan — Present Findings, Ask User

Compile the findings into a refactoring plan:

```text
## Refactoring Report: <target>

### Baseline
- Tests: <pass/fail before starting>
- Target: <file(s) under review>

### Bad Smells Found
| # | File:Line | Severity | Smell | Proposed Fix |
|---|-----------|----------|-------|-------------|
| 1 | path.rs:42 | HIGH | unwrap() in production | Replace with `?` + anyhow context |
| 2 | path.rs:88 | MED | Duplicate block (lines 85-95 and 120-130) | Extract into `fn normalize()` |

### Proposed Changes
1. <change 1 — one-liner>
2. <change 2 — one-liner>
3. <change 3 — one-liner>

### Risk Assessment
- Low risk: <changes that are purely mechanical>
- Medium risk: <changes that touch shared interfaces>
- High risk: <changes near Rust/Lua boundary or CLI output>

### Estimated Test Commands
```
cargo test -- -D warnings
tests/run.sh
```

---

**Action required:** Review the report above. Reply with:
- `proceed` — execute all proposed changes
- `proceed N,M` — execute only changes N and M
- `skip` — abandon refactoring
- modify the plan — describe changes to the plan
```

Present this report to the user and wait for a response. Do not start editing code until the user confirms.

### Phase 3: Execute — One Change At A Time

Only proceed after user approval. Changes must be made one conceptual unit at a time.

1. After approval, run baseline tests if not already done in Phase 1.
2. Implement the first approved change.
3. Run `cargo check` and the relevant test suite.
4. Only proceed to the next change when tests pass.

```bash
cargo check 2>&1
cargo test -- -D warnings      # full test suite
```

If a change causes test failures, revert it, report the failure to the user, and ask whether to skip or fix.

### Phase 4: Verify No Behavior Change

Beyond tests, verify:

- `cargo run -- run examples/demo.http --env dev` produces the same output (functional check for Rust)
- `cargo run -- context detect "SELECT * FROM " 14` produces the same JSON (for SQL context changes)
- Lua-side: open a test buffer in Neovim and visually confirm completion, dataset rendering, and navigation work as before

Use diff-based verification for critical refactors:

```bash
# Before refactor
cargo build && cargo run -- run tests/sql/queries/postgres.sql --line 4 --env dev > /tmp/output-before.txt

# After refactor
cargo build && cargo run -- run tests/sql/queries/postgres.sql --line 4 --env dev > /tmp/output-after.txt

diff /tmp/output-before.txt /tmp/output-after.txt
```

### Phase 5: Final Validation

Run the full test suite one last time and present a summary to the user:

```text
## Refactoring Complete: <target>

### Changes Made
1. <change 1 — brief>
2. <change 2 — brief>

### Test Results
- `cargo test -- -D warnings`: PASS
- `tests/run.sh`: PASS
- `cargo clippy -- -D warnings`: PASS
- Diff verification: no output changes

### Residual Risk
<none, or list of known low-risk gaps>
```

## Rust Refactoring Patterns

### Module Splitting

When splitting a large module (`mod.rs` or single file) into multiple files:

1. Create new files in the same directory.
2. Add `mod new_file;` and `pub use new_file::...;` in `mod.rs` to preserve the public API.
3. Move code in logical groups, keeping `pub` visibility for items consumed by other modules.
4. Move tests last — they may depend on `super::*` or `use crate::...;`.
5. Run `cargo test` after each file move.

### Renaming

For type/function renames:

1. Change the definition.
2. Update all references — use `cargo check` to find remaining references.
3. Update doc comments referencing the old name.
4. Update test code.
5. If the name appears in serde `#[serde(rename = "...")]`, the serialized field name does NOT change — only the Rust identifier changes.

### Removing `unwrap()`

Replace with:

```rust
// Before
let x = foo().unwrap();

// After
let x = foo()?;                     // if the function returns Result
let x = foo().context("...")?;      // with anyhow context
let x = foo().expect("reason");     // only for test code or infallible paths
```

Do NOT change `unwrap()` if:
- The code is in a `Drop` impl or destructor where `?` cannot be used
- The `unwrap()` is in test code and is the idiomatic pattern there

### Error Type Migration

When changing error types:

1. Implement `From<OldError> for NewError` to avoid breaking every call site at once.
2. Update call sites incrementally.
3. Remove the `From` impl once all call sites are updated.

## Lua Refactoring Patterns

### Module Splitting

Lua modules in this project follow the `local M = {} ... return M` pattern:

```lua
-- Before: everything in one file
local M = {}

function M.foo() end
function M.bar() end

return M
```

```lua
-- After: split into two files, preserving API
-- file_a.lua
local M = {}
function M.foo() end
return M

-- file_b.lua
local M = {}
function M.bar() end
return M

-- parent/init.lua
local M = {}
M.foo = require("poste.parent.file_a").foo
M.bar = require("poste.parent.file_b").bar
return M
```

### Renaming Local Functions

1. Rename the definition.
2. Search for all uses of the old name within the same file.
3. Update internal callers.
4. If the function is exported via `M.fn`, the external name is unchanged — only the internal name changes.

### Removing Dead Code

1. Search for references to the identifier across the Lua codebase.
2. If unused, remove the entire function/variable block.
3. Run `tests/run.sh` to confirm.

## What To Test After Refactoring

### Mandatory

```bash
cargo test -- -D warnings
tests/run.sh
```

### Rust-Specific

```bash
cargo clippy -- -D warnings      # no new warnings from refactoring
cargo check                      # also checks doc tests and examples
```

### When SQL Completion Code Is Touched

```bash
# Test all three completion modes (see sql-completion skill)
cargo test -p poste-core
printf 'SELECT * FROM users WHERE ' | cargo run -- context detect 26
```

### When Lua Side Is Touched

Run the Lua test suite and manually smoke-test the affected area in Neovim:

- For `lua/poste/` changes: open an `.http` file, trigger completion, run a request
- For `lua/poste/sql/` changes: open a `.sql` file, trigger completion, run a query, verify dataset results

### When Shared State (`state.lua`) Is Touched

- Test both HTTP and SQL workflows
- Check that `.http` and `.sql` namespace isolation is preserved

## Common Pitfalls

### Missing Re-exports

After splitting a Rust module, callers in other crates may rely on `use crate::foo::Bar` resolving through `mod.rs`. If `Bar` was moved but not re-exported, those callers break silently until `cargo check`.

Fix: add `pub use submodule::Bar;` in the parent `mod.rs`.

### Serde Field Name Changes

Renaming a Rust struct field that uses `#[serde(rename = "json_name")]` changes the JSON key that Lua consumes. If the rename must happen, update the Lua consumer in the same change.

### Lua `require()` Path Breakage

When reorganizing `lua/poste/sql/` files, every file that `require("poste.sql.foo")` must be updated. Search for `require("poste.sql.` strings to find all consumers.

### Crate Dependency Changes

When moving code between Rust crates, the target crate must list the source crate's dependencies in its own `Cargo.toml`. `cargo check` will catch this, but it can still be surprising when a "pure refactor" requires adding deps.

### Test Isolation Breaks

When Lua test files reference specific module paths, reorganizing modules can break test `require()` calls. Update test imports to match new paths, but do not change the assertions.

### Protocol Isolation

HTTP and SQL refactoring must stay within their respective crate/module boundaries:

- `crates/poste-core/src/parser.rs` changes may affect HTTP and SQL — test both
- `crates/poste-exec/src/executor.rs` is HTTP-only — do not introduce SQL concerns
- `crates/poste-exec/src/sql_executor.rs` is SQL-only
- Lua `lua/poste/` (non-sql) vs `lua/poste/sql/` — keep modules separate

## Validation Checklist

- [ ] Bad smell scan completed before any code changes
- [ ] Refactoring report presented to user and approved
- [ ] `cargo test -- -D warnings` passes (same as before refactoring)
- [ ] `tests/run.sh` passes (Lua tests)
- [ ] `cargo clippy -- -D warnings` has no new warnings
- [ ] No new `#[allow(...)]` attributes added (unless truly unavoidable)
- [ ] Public APIs (Rust `pub`, Lua `M.*`) remain unchanged in name and shape
- [ ] CLI output format and JSON response fields are identical
- [ ] No new external dependencies added
- [ ] No dead code left behind (unless explicitly marked with a reason)
- [ ] HTTP/SQL isolation boundaries respected

## Do Not

- Do not combine refactoring with feature work in the same change
- Do not "clean up" unrelated code paths — scope creep breaks the diff
- Do not reformat entire files unless the project formatter is part of the project's standard workflow
- Do not add comments, docs, or type annotations as part of a refactoring-only task
- Do not introduce new abstractions (traits, interfaces, base classes) unless the refactoring specifically calls for it
- Do not change `Cargo.toml` dependency versions unless the refactoring requires an API from the newer version
- Do not remove `#[cfg(test)]` imports or test helpers that other tests depend on
- Do not assume a code path is dead without verifying with `rg` and the test suite
- Do not leave commented-out code — either keep it or remove it
- Do not skip the bad smell identification and reporting phases — present findings before editing
- Do not start editing without user approval of the refactoring plan
