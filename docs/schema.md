# `poste` binary contract (the family's only cross-repo interface)

Since the poste.nvim family dissolution (2026-09-13) the sibling plugins —
[poste-db.nvim](https://github.com/beyondlex/poste-db.nvim),
[poste-redis.nvim](https://github.com/beyondlex/poste-redis.nvim),
[poste-es.nvim](https://github.com/beyondlex/poste-es.nvim),
[poste-mail.nvim](https://github.com/beyondlex/poste-mail.nvim), … — are
self-contained Neovim plugins. They vendor their own Lua; **the one thing they
all still share is this binary**. Its CLI surface and wire formats are a
compatibility contract: a change here is a breaking change for every sibling
and must ship as a binary MAJOR (or be additive).

Siblings resolve the binary via their vendored `find_poste_binary()`:
`vim.g.poste_binary` → their own `config.poste_binary` (default
`stdpath("data")/poste/bin/poste`, managed by the vendored installer, which
still downloads from THIS repo's releases) → `target/{debug,release}/poste`
next to cwd or the plugin dir → `poste` on PATH.

## Global conventions

- `--connection` always takes an already-resolved **URL** (e.g.
  `postgres://user:pass@host/db`), never a connection NAME. Name → URL
  resolution is Lua-side (each sibling reads `connections.toml` itself).
- `--env <name>` selects the environment for `{{var}}` expansion where
  supported (default `dev`).
- `--database <name>` overrides the connection's default database where
  supported.
- SQL/Redis/AMQP engines never read config files; Lua hands them fully
  resolved parameters.
- `poste --version` prints `poste <tag> (<build-date>)`; siblings' health
  checks display it. Record the version you tested against there.

## Connection-name resolution (mirror implementations — do not drift alone)

Lua (e.g. `poste-db/lua/poste-db/connections.lua` `resolve_connection_url`)
and Rust (`crates/poste-exec/src/sql_connection.rs`
`ConnectionConfig::to_url()`) are **mirror implementations** of the same
rules. Any change must be applied to every mirror in the same release:

1. `connections.toml` (walked up from the file/buffer directory), section
   `[<name>]`; the sibling only enumerates its own dialect sections.
2. `{{VAR}}` references expand from: OS env > `.env` (walk-up) > `env.json`
   (`{"envs": {"dev": {...}}}` or flat). Unknown vars stay literal.
3. Field-form connections build a URL: `dialect` scheme + `host`/`port`/
   `database`/`user`/`password` (+ percent-encoding of reserved bytes).
   `dialect` aliases (`postgresql`→`postgres`, `mariadb`→`mysql`, …)
   normalize to base dialects first.

## Subcommands

### `exec-file` — one-shot SQL file execution (poste-db)

```
poste exec-file <file> [--connection URL] [--database NAME] [--env NAME]
                [--mode transaction|greedy] [--timeout S] [--max-rows N] [--json]
```

Parses the `.sql` file itself (`###` section markers, `-- @connection: NAME`
/ `-- @database: NAME` directives are stripped Lua-side where applicable).
`--json` streams NDJSON progress/result events on stdout; per-statement
objects carry `seq`, `sql`, `status`, `latency_ms`, and for SELECTs a
`resultset` (`columns[].name`, `rows` as arrays, `row_count`, `total_rows`).
MySQL `BINARY`/`BLOB` values are emitted as uppercase hex strings.

Exit code 0 even for per-statement SQL errors in `greedy` mode (errors are
events); non-zero for transport-level failures.

### `session` — persistent SQL session (poste-db)

```
poste session --connection URL [--database NAME] [--timeout S] [--max-rows N]
```

stdin/stdout NDJSON loop. Request: `{"seq": <u64>, "sql": "<string>"}`.
Events: progress / `{"type":"result","seq":N,"status":"ok|error",...}`
(same resultset shape as `exec-file`; session events carry no `total`
key and error events report real elapsed). Both transports share ONE
implementation of statement classification, truncation and value
conversion (`poste-exec::sql_exec_common` / `sql_values`) — the old
"session is the live value converter, exec-file must stay in sync"
discipline is enforced by construction now. The process keeps the
connection open; `USE <db>` and `SELECT` state persist across requests.

### `redis-exec` — one-shot pre-tokenized commands (poste-redis)

```
echo '{"connection":"redis://...","commands":[["GET","k"],["SET","k","v"]]}'
  | poste redis-exec
```

stdin is ONE JSON object: `{ connection: string, commands: string[][] }` —
Lua parses the `.redis` buffer; the binary never sees file text. stdout is
NDJSON: `progress` / `result` (`status` is redis-cli style text; a failed
command yields `status:"error"` + `error`, the batch continues — greedy) /
`summary` (total/failed/elapsed).

### `redis-session` — persistent redis session (poste-redis)

```
poste redis-session --connection redis://... [--max-items N] [--max-bytes N]
```

stdin lines: `{"seq": <u64>, "command": ["GET", "k"]}`. NDJSON result events
per line; one multiplexed connection, `SELECT` db state persists (Lua steers
it with fire-and-forget SELECTs). Array/map replies are capped by
`--max-items`/`--max-bytes` (excess marked truncated).

### `mq-exec` — one-shot pre-decoded AMQP operations (poste-mq)

```
echo '{"connection":"amqp://...","operations":[...]}' | poste mq-exec
```

stdin: `{ connection: string, operations: object[] }`; NDJSON progress /
result / summary events on stdout, greedy like `redis-exec`.

### `mq-session` — persistent AMQP session with push consumers (poste-mq)

```
poste mq-session --connection amqp://...
```

stdin/stdout control loop; consumer watch uses a bounded prefetch (500) so
ack-mode consumers get backpressure and requeue-mode closes batch-nack the
held deliveries.

### `introspect` — database structure introspection (poste-db)

```
poste introspect --connection-url URL --type databases|schemas|tables|columns|indexes
                 [--schema NAME] [--table NAME] [--database NAME]
```

`--connection-url` is Lua-resolved (URL, not a name). JSON result on stdout;
table/column/index queries are schema-scoped (PG search path honored).

### `context` — SQL text analysis (poste-db completion/indicators)

```
poste context detect <offset> [--dialect generic|postgres|mysql|sqlite]
poste context stmt   <offset> # statement boundaries around a cursor line
poste context stmt-ranges     # ALL statement boundary line ranges in the text
```

Pure text analysis: reads SQL text on stdin, answers completion-context /
statement-boundary questions for the given 0-based byte `offset`. `stmt`
returns `{start_line, end_line}` (0-based); `stmt-ranges` returns
`[[start, end], ...]` pairs covering every statement in the text.

### `connection` — connections.toml inspection helpers

```
poste connection list [--path DIR] [--env NAME] [--json]
```

Lists resolved connections (name → URL with `{{var}}` expansion applied).
Siblings primarily resolve names themselves; this subcommand is a
debug/parity helper against the Rust resolver.

## Stability discipline

- additive CLI flags / event fields → MINOR (siblings must tolerate unknown
  fields — parse leniently);
- removing/renaming flags, changing event shapes or the mirror resolution
  rules → MAJOR, shipped in lockstep with sibling updates;
- when Lua and the binary disagree, first check WHICH binary you are running
  (`poste --version`, `vim.g.poste_binary`): an unreleased dialect worktree
  (`../poste-for-db`) is a common source of skew.
