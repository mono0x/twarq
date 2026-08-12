# AGENTS.md

## Overview / usage

See `README.md`. The CLI surface is defined in `src/cli.rs` (clap derive). The
project name is `twarq` even though the repository directory is `xarq`.

## Toolchain and commands

- Rust version: pinned in `mise.toml` (`mise install` if missing).
- Formatting and linting go through hk (`hk.pkl`): `hk fix` to format/fix, `hk check` to verify (both default to changed files, `--all` for everything).
- clippy is deliberately not part of hk (it would force a full bundled-DuckDB build on every commit). Run `cargo clippy --all-targets --all-features -- -D warnings` yourself; CI runs it in the `test` job.
- DuckDB linkage is feature-gated: the default `bundled` feature compiles and statically links DuckDB (what releases ship). CI's `test` job instead sets `DUCKDB_DOWNLOAD_LIB=1` and builds with `--no-default-features`, dynamically linking the official prebuilt libduckdb to skip the multi-minute C++ build; the bundled configuration is only exercised by the release build.
- Test commands and the OS × target matrix: `.github/workflows/ci.yml` is the source of truth — mirror those locally rather than maintaining a duplicate list here.
- Tests are colocated in `src/main.rs` under `mod tests`. Run a single test with `cargo test <fn_name>`. The first build compiles bundled DuckDB and takes several minutes.

## Architecture (non-obvious points)

Modules under `src/` (`cli.rs`, `import.rs`, `query.rs`, `search.rs`, `main.rs`) are small enough that reading them is faster than reading a description. The points below are the things that are _not_ obvious from the code alone.

### Output serialization happens in SQL, not Rust

`run_query` in `src/query.rs` wraps every statement in `SELECT to_json(t) FROM (...) t` (jsonl/json) or `COLUMNS(*)::VARCHAR` (table), so DuckDB serializes lists, timestamps and NULLs and Rust only ever reads strings. This is why there is no chrono dependency and no `Value` tree walking. If you add a new output path, keep it on this pattern.

### Import goes appender → staging table → one INSERT ... SELECT

Per-row prepared-statement INSERTs made a real archive take hours (DuckDB
executes a full query pipeline per statement, and without a transaction each
one auto-commits and fsyncs the WAL). `src/import.rs` instead appends rows to
a `TEMP` staging table with the appender API, then converts and inserts the
whole batch in a single vectorized `INSERT OR IGNORE ... SELECT`, all inside
one transaction per archive (~seconds for a 5 GB archive). Keep new import
paths on this pattern; do not add per-row statements.

### Row-level failures must not raise statement errors

A statement error inside a DuckDB transaction poisons it: every later
statement fails and the COMMIT silently discards the whole archive. So
malformed rows are dropped inside the SQL itself — `try_strptime` returns
NULL for an unparseable `created_at` and the `WHERE ... IS NOT NULL` clause
skips the row (counted and logged afterwards by querying the staging table).
A genuine statement error propagates and rolls back the archive's transaction
rather than being skipped.

### VARCHAR[] binding goes through from_json

duckdb-rs cannot bind a Rust `Vec` to a `VARCHAR[]` column. The staging table
stores entity lists as JSON-encoded VARCHAR and the INSERT ... SELECT converts
with `from_json(col::JSON, '["VARCHAR"]')`. This depends on the JSON
extension: the `json` cargo feature statically links it in bundled builds, and
the official prebuilt libduckdb (non-bundled CI builds) already includes it.

### created_at parsing avoids %z on purpose

The tweet INSERT parses `created_at` with a `try_strptime` format that matches
the `+0000` offset _literally_. Using `%z` would make strptime return
TIMESTAMPTZ, and both the cast back to TIMESTAMP and `SET TimeZone` require
DuckDB's icu extension, which is not statically linked (and autoloading would
hit the network). Archive timestamps are always `+0000`, so the literal match
is safe; anything else yields NULL and the row is skipped and counted.

### No full-text search index, deliberately

DuckDB's `fts` extension tokenizes on whitespace, so Japanese collapses into one token per tweet. `search` uses `ILIKE` substring scans instead; at archive scale (~tens of thousands of rows) this is milliseconds. Do not add fts or semantic search.
