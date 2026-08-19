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

Or download a binary from the releases page.

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

Main options:

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

### MCP server

```sh
twarq mcp --db archive.duckdb                        # stdio
twarq mcp --db archive.duckdb --http 127.0.0.1:8080  # Streamable HTTP at /mcp
```

Runs an MCP server exposing `search_tweets`, `search_likes`, `query` and
`schema` as tools, so MCP clients such as Claude Code can search the
archive. The database is opened read-only.

```json
{
  "mcpServers": {
    "twarq": {
      "command": "twarq",
      "args": ["mcp", "--db", "/path/to/archive.duckdb"]
    }
  }
}
```

With `--http` the same tools are served over Streamable HTTP at
`http://<addr>/mcp` (`"url"` instead of `"command"` in the client config).
The HTTP mode is stateless — no session handling — and only accepts
requests whose `Host` header is a loopback address, which is what protects
a locally running server from DNS rebinding. Put it behind a reverse proxy
or a tunnel if you need to reach it from another machine.

## Database schema

Terminology follows the archive format itself: an _account_ is an archive
owner imported into the database (`data/account.js`), while _user_ refers to
any Twitter user referenced only by ID, such as reply targets and followers.

### accounts

| column         | type    | description              |
| -------------- | ------- | ------------------------ |
| `account_id`   | VARCHAR | account ID (primary key) |
| `username`     | VARCHAR | screen name              |
| `display_name` | VARCHAR | display name             |

### tweets

Primary key is `(account_id, id)`.

| column                    | type      | description                                                    |
| ------------------------- | --------- | -------------------------------------------------------------- |
| `account_id`              | VARCHAR   | owning account                                                 |
| `id`                      | VARCHAR   | tweet ID                                                       |
| `text`                    | TEXT      | tweet body; links stay as `t.co` short URLs                    |
| `created_at`              | TIMESTAMP | creation time (UTC)                                            |
| `retweet_count`           | INTEGER   |                                                                |
| `favorite_count`          | INTEGER   |                                                                |
| `source`                  | TEXT      | posting application (HTML anchor tag)                          |
| `lang`                    | VARCHAR   | language code                                                  |
| `deleted`                 | BOOLEAN   | true for tweets from `deleted-tweets.js`                       |
| `in_reply_to_status_id`   | VARCHAR   |                                                                |
| `in_reply_to_user_id`     | VARCHAR   |                                                                |
| `in_reply_to_screen_name` | VARCHAR   |                                                                |
| `urls`                    | VARCHAR[] | expanded URLs (not `t.co`)                                     |
| `hashtags`                | VARCHAR[] | hashtags without `#`                                           |
| `mentions`                | VARCHAR[] | mentioned screen names without `@`                             |
| `media_urls`              | VARCHAR[] | image/video URLs                                               |
| `media_types`             | VARCHAR[] | `photo` / `video` / `animated_gif`, same order as `media_urls` |

### likes

Primary key is `(account_id, tweet_id)`.

| column       | type    | description                      |
| ------------ | ------- | -------------------------------- |
| `account_id` | VARCHAR | account that liked the tweet     |
| `tweet_id`   | VARCHAR | liked tweet ID                   |
| `text`       | TEXT    | liked tweet's text (may be NULL) |
| `url`        | TEXT    | link to the tweet                |

### followers / following

Primary key is `(account_id, follower_id)` / `(account_id, following_id)`.
The archive contains only user IDs and links, not screen names.

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
