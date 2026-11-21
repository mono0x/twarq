# AGENTS.md

## Overview / usage

See [README.md](README.md) for usage and documentation links. The CLI surface
is defined in `src/cli.rs` (clap derive). The project name is `twarq`.

## Toolchain and commands

- Rust version: pinned in `mise.toml` (`mise install` if missing).
- Formatting and linting go through hk (`hk.pkl`): `hk fix` to format/fix, `hk check` to verify (both default to changed files, `--all` for everything).
- clippy is not part of hk. Run `cargo clippy --all-targets --all-features -- -D warnings` yourself; CI runs it in the `test` job.
- Rust/Clippy lint policy lives in `Cargo.toml`: standard Clippy lints plus selected checks for lossy numeric casts, unnecessary ownership transfers and string allocations. Do not enable the entire `pedantic` group. Unsafe code, ignored must-use results, `dbg!`, `todo!`, `unimplemented!`, and `unwrap()` are denied (`unwrap()` is allowed in tests). Use `expect()` only for invariants, with an explanatory message. Keep lint exceptions local and reasoned; prefer `#[expect(..., reason = "...")]` so stale exceptions fail.
- SQLite is compiled from source and statically linked through rusqlite's `bundled` feature, so every build (local, CI and release) uses the same SQLite and there is no system library dependency.
- Test commands and the OS × target matrix: `.github/workflows/ci.yml` is the source of truth — mirror those locally rather than maintaining a duplicate list here.
- Tests are colocated in `src/main.rs` under `mod tests`. Run a single test with `cargo test <fn_name>`.

## Architecture (non-obvious points)

Modules under `src/` (`cli.rs`, `import.rs`, `query.rs`, `search.rs`, `main.rs`) are small enough that reading them is faster than reading a description. The points below are the things that are _not_ obvious from the code alone.

### Output serialization relies on declared column types

`run_query` in `src/query.rs` converts each SQLite value to JSON in Rust. SQLite has no list or boolean types, so the schema declares list columns as `JSON` (JSON text) and flags as `BOOLEAN` (0/1), and `to_json` uses the column's declared type to emit real arrays and booleans. Declared types survive subqueries and joins but not expressions. If you add a list or flag column, declare it with one of these types.

### Import is one transaction per archive

`src/import.rs` inserts rows with prepared statements inside one transaction per archive; without the transaction every INSERT auto-commits and syncs the journal. Malformed rows (an unparseable `created_at`) are skipped and counted in Rust before reaching SQL. A genuine statement error propagates and rolls back the archive's transaction, so an archive is imported entirely or not at all.

### created_at is stored as UTC text

The archive's `created_at` (`Mon Sep 24 03:35:21 +0000 2018`) is converted in Rust to `2018-09-24 03:35:21`, which SQLite's date functions accept and which sorts chronologically as text. Archive timestamps are always `+0000`; anything else is treated as malformed rather than converted, so no date/time crate is needed.

### No full-text index, deliberately

SQLite's FTS5 tokenizes on whitespace, so Japanese collapses into one token per sentence. `search` uses `LIKE` substring scans instead; at archive scale (~tens of thousands of rows) this is milliseconds. Do not add FTS or semantic search.
