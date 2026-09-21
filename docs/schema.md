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
rules. Any change must be applied to every mirror in the same release. Not
every rule lives in those two files — rules 4, 5 and 6 have their own single
home per side, named below, and are mirrors of each other too:

1. `connections.toml` (walked up from the file/buffer directory), section
   `[<name>]`; the sibling only enumerates its own dialect sections.
2. `{{VAR}}` references expand from: OS env > `.env` (walk-up) > `env.json`
   (`{"envs": {"dev": {...}}}` or flat). Unknown vars stay literal. The four
   details both sides must keep: a reference name is **anything up to the
   closing brace pair** (`[^}]+` / `(.-)`, not an identifier pattern, because
   env.json keys are arbitrary strings); an env.json section contributes only
   **scalars** — string, number and boolean become text, objects and null are
   not variables (a table in the substitution table aborts Lua's `gsub`); the
   **flat form is read when the requested stanza is absent**, so a file with
   no `dev` key still supplies vars for every environment; and expansion
   repeats to a **fixed point, capped at 10 rounds**, so a value that itself
   references a var resolves and a cycle ends unresolved rather than hanging.
3. Field-form connections build a URL: `dialect` scheme + `host`/`port`/
   `database`/`user`/`password` (+ percent-encoding of reserved bytes).
   `dialect` aliases (`postgresql`→`postgres`, `mariadb`→`mysql`, …)
   normalize to base dialects first. `port` expands `{{VAR}}` like the
   string fields — the value is only known after the environment is applied
   — and must then be an integer in 1–65535; otherwise that one connection
   errors while the rest of the file still resolves (Rust carries the
   unparseable value in `ConnectionConfig::port_raw` until then).
4. Scheme → protocol sniffing: Lua's `constants.URL_SCHEMES` and Rust's
   `poste_core::Protocol::from_sql_url` (the one chain `exec-file`, `session`
   and `introspect` share) accept the same prefixes — `sqlite:`,
   `postgres://` + `postgresql://`, `mysql://` + `mariadb://`, `mssql://`,
   `clickhouse://` — and nothing else. The alias schemes matter only for a
   raw `url = "…"` entry, which bypasses rule 3's normalization.
5. A message that quotes a resolved URL masks its password
   (`poste_core::mask_url_password`). The URL is the credential carrier; the
   connection *name* is what may be printed. The scan is two-step because one
   pass cannot serve both shapes: an `@` inside the authority is the userinfo
   separator, and a `@` past the first `/` only is when the authority itself
   already looks like credentials (`user:secret`, not `host:5432`) — that
   catches a hand-written `url` whose password holds an unencoded `/`, while
   leaving a legal `@` in a database name (`/team@billing`) alone.
6. A `-- @<name> <value>` directive owns its line: `^\s*--\s*@name\s+(.+)`
   (Rust `extract_connection_directive` / `strip_sql_directives`, Lua
   `constants.match_directive`, which `file_exec.lua` and the completion
   handlers call). Nothing trailing a statement counts — a `-- @connection …`
   after a `;` would otherwise steer which database the file runs against
   while staying inside the SQL body, and a `'-- @connection …'` inside a
   string literal would suppress the header the AI actions write.

## Subcommands

### `exec-file` — one-shot SQL file execution (poste-db)

```
poste exec-file <file> [--connection URL] [--database NAME] [--env NAME]
                [--mode transaction|greedy] [--timeout S] [--max-rows N] [--json]
```

Parses the `.sql` file itself (`###` section markers; `-- @connection <URL>`
and `-- @database <name>` lines follow rule 6 and are read and stripped here,
not left to the editor). `--json` streams NDJSON progress/result events on
stdout; per-statement objects carry `seq`, `sql`, `status`, `latency_ms`, and
for SELECTs a `resultset` (`columns[].name`, `rows` as arrays, `row_count`,
`total_rows`). MySQL `BINARY`/`BLOB` values are emitted as uppercase hex
strings.

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

stdin lines: `{"seq": <u64>, "command": ["GET", "k"]}`, where `command` is a
non-empty array of **strings** — a missing array, a non-string token or an
empty command is rejected as an error event carrying that `seq`, never dropped
silently (the caller registers its pending callback before sending, so a
request that goes unanswered stalls until its watchdog restarts a healthy
session). NDJSON result events per line; one multiplexed connection, `SELECT`
db state persists, and Lua steers it with its own tracked `SELECT` request (a
failed `SELECT` errors the command behind it rather than running it on the old
database — including after a reconnect, whose restore is bounded and whose
failure is reported instead of retried on db 0). Array/map replies are capped
by `--max-items`/`--max-bytes` (excess marked truncated).

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

stdin/stdout control loop — `{"consumer": NAME, "action": "start", "queue":
Q, "ack": bool}` then `{"consumer": NAME, "action": "stop"}`. `start` requires
a non-empty `queue` (an empty one would make AMQP mint a server-named queue
that nothing publishes to) and a boolean `ack` (`"yes"` is rejected rather
than read as `false`); `stop` reports whether a consumer was actually torn
down. Consumer watch uses a bounded prefetch (500) so ack-mode consumers get
backpressure and requeue-mode closes batch-nack the held deliveries.

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

What the answers mean, since three scanners (the splitter, the classifier and
this tokenizer) read the same text and must not disagree:

- A statement runs across set operators: a statement keyword preceded, at the
  same paren depth, by `UNION`/`INTERSECT`/`EXCEPT` (optionally `ALL` or
  `DISTINCT`) is an arm of the same statement, not a boundary. `MINUS` is not
  one — it is Oracle-only, and treating it as an operator would merge
  statements for everyone else.
- `tables` lists the relations visible to the *cursor's* query block: one
  frame per paren level, resolved innermost first with its ancestors, so a
  subquery's columns cannot leak outward and a `WITH` name stays visible
  across every arm. Comma-separated FROM elements, `LATERAL`/`ONLY`
  qualifiers and table functions are all parsed.
- `prefix` is authoritative. The caller must not re-derive it from the text
  before the cursor: only this side knows a quoted qualifier from an
  unterminated one (`"p"."ti` while typing) and what its last character was.
- A cursor one past the final character of an identifier — the position the
  editor reports — counts as being *inside* that word.
- Literal and identifier quoting follows `--dialect`: MySQL reads `\'` as an
  escape, every other dialect reads it literally; `E'…'` opts into escapes in
  any dialect; a doubled quote ends a run only where it really does — `''`
  inside strings, the same rule for `"` and the backtick forms, and Postgres
  `$tag$…$tag$` dollar quotes closing on their own tag.
- `functions` is the dialect's own menu (`known_functions_for_dialect`), so a
  SQLite buffer sees the functions SQLite shares with MySQL.

### `connection` — connections.toml inspection helpers

```
poste connection list [--path DIR] [--env NAME] [--json]
```

Lists resolved connections (name → URL with `{{var}}` expansion applied).
Siblings primarily resolve names themselves; this subcommand is a
debug/parity helper against the Rust resolver. The Rust store reads
`connections.toml` (walking up from `--path`, same discovery as the
siblings — TOML wins when a directory carries both files) with a legacy
`connections.json` fallback; `poste connection test <name>` resolves
through the same store.

## Stability discipline

- additive CLI flags / event fields → MINOR (siblings must tolerate unknown
  fields — parse leniently);
- removing/renaming flags, changing event shapes or the mirror resolution
  rules → MAJOR, shipped in lockstep with sibling updates;
- when Lua and the binary disagree, first check WHICH binary you are running
  (`poste --version`, `vim.g.poste_binary`): an unreleased dialect worktree
  (`../poste-for-db`) is a common source of skew.
