# twarq

Import X (Twitter) archives into DuckDB and search them from the command line.

`twarq` extracts tweets, deleted tweets, likes, followers and following from
X archive ZIP files (the 2025+ format) into a single DuckDB database. Archives
from multiple accounts can be imported into the same database and searched
across. Output is JSON Lines by default, intended for consumption by AI agents
(Claude Code etc.) as well as humans.

Inspired by [tweetduck](https://github.com/mizzy/tweetduck); the main
differences are multi-account support and likes.

## Install

```sh
cargo install --git https://github.com/mono0x/twarq
```

Or download a binary from the [releases page](https://github.com/mono0x/twarq/releases).

## Documentation

- [CLI usage](#usage): import archives, search tweets and likes, and run SQL.
- [Database schema](docs/schema.md): tables, columns and primary keys.
- [MCP setup](docs/mcp.md): connect a client over stdio or Streamable HTTP.
- [Releasing](docs/releasing.md): publish a release and retry failed runs.
- [Development notes](AGENTS.md): toolchain, checks and implementation constraints.

## Usage

### Import

```sh
twarq import twitter-archive.zip
twarq import alice.zip bob.zip --db archive.duckdb   # multiple accounts
twarq import archive.zip --verbose                   # per-file progress
```

Each archive is keyed by the account it belongs to (`data/account.js`), so
archives from different accounts coexist in one database. Re-importing the
same archive adds no duplicates. Split data files (`tweets-part1.js` etc.) are
picked up automatically. The default database path is `twarq.duckdb`.

### Search

```sh
twarq search 旅行                            # keyword search (all accounts)
twarq search 旅行 京都                       # multiple keywords are ANDed
twarq search --account alice --since 2025-01-01
twarq search --reply-to bob -n 50
twarq search --likes 旅行                    # search likes instead of tweets
twarq search 旅行 --format table             # for human reading
```

Keywords are case-insensitive substring matches, so Japanese works as-is.

Run `twarq search --help` for all options. Main options:

- `--account`: only tweets/likes of this username
- `--likes`: search likes; keywords match the liked tweet's text
- `--since` / `--until`: date range (YYYY-MM-DD, inclusive on both ends)
- `--mentions` / `--reply-to`: mentioned / replied-to screen name
- `--has-media` / `--has-url` / `--no-retweets`
- `--include-deleted` / `--only-deleted`: deleted tweets (excluded by default)
- `--oldest-first`: oldest first (default is newest first)
- `-n`, `--limit`: maximum rows (default 20)
- `-f`, `--format`: `jsonl` (default) / `json` / `table`

### SQL queries

See the [database schema](docs/schema.md) for tables and columns.

```sh
twarq query "SELECT text, created_at FROM tweets WHERE lang = 'ja'" -n 5

# Tweets per year per account (use -n 0 to lift the automatic cap)
twarq query "SELECT a.username, year(t.created_at) y, count(*) n
             FROM tweets t JOIN accounts a USING (account_id)
             GROUP BY ALL ORDER BY 1, 2" -n 0 -f table

# Most-used hashtags (unnest expands array columns)
twarq query "SELECT tag, count(*) n FROM (SELECT unnest(hashtags) tag FROM tweets)
             GROUP BY tag ORDER BY n DESC" -n 10
```

The database is opened read-only, so `DELETE` and `DROP` are rejected by
DuckDB itself. Results are capped at `-n` rows (default 100, `0` disables).

### Schema

```sh
twarq schema
```

Shows tables, columns and row counts for the selected database.

## Notes

- **No full-text index.** DuckDB's `fts` extension tokenizes on whitespace, so
  Japanese text collapses into one token per tweet. At archive scale a
  substring scan takes milliseconds, so `search` uses `ILIKE`.
- **Retweets are stored with the body starting `RT @user:`.** Use
  `--no-retweets` or `text NOT LIKE 'RT @%'` to filter them.
- **Archives contain only your own tweets.** Replies from others are not
  included, so threads can only be reconstructed from your side.
- **Likes have no timestamp** in the archive; `search --likes` orders by the
  numeric tweet ID, which approximates recency.
- Deleted tweets share the `tweets` table with `deleted = true`. `search`
  excludes them by default, but in `query` you need `WHERE NOT deleted`
  explicitly.

## License

MIT License
