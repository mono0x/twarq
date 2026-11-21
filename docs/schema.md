# Database schema

Terminology follows the archive format itself: an _account_ is an archive
owner imported into the database (`data/account.js`), while _user_ refers to
any Twitter user referenced only by ID, such as reply targets and followers.

The database is SQLite. It has no list or boolean types, so list columns
(declared `JSON`) hold JSON arrays — expand them with `json_each` — and
`deleted` (declared `BOOLEAN`) holds 0 or 1. Query output still renders them
as JSON arrays and true/false when they are selected directly.

## accounts

| column         | type | description              |
| -------------- | ---- | ------------------------ |
| `account_id`   | TEXT | account ID (primary key) |
| `username`     | TEXT | screen name              |
| `display_name` | TEXT | display name             |

## tweets

Primary key is `(account_id, id)`.

| column                    | type    | description                                                    |
| ------------------------- | ------- | -------------------------------------------------------------- |
| `account_id`              | TEXT    | owning account                                                 |
| `id`                      | TEXT    | tweet ID                                                       |
| `text`                    | TEXT    | tweet body; links stay as `t.co` short URLs                    |
| `created_at`              | TEXT    | creation time in UTC, `YYYY-MM-DD HH:MM:SS`                    |
| `retweet_count`           | INTEGER |                                                                |
| `favorite_count`          | INTEGER |                                                                |
| `source`                  | TEXT    | posting application (HTML anchor tag)                          |
| `lang`                    | TEXT    | language code                                                  |
| `deleted`                 | BOOLEAN | 1 for tweets from `deleted-tweets.js`                          |
| `in_reply_to_status_id`   | TEXT    |                                                                |
| `in_reply_to_user_id`     | TEXT    |                                                                |
| `in_reply_to_screen_name` | TEXT    |                                                                |
| `urls`                    | JSON    | expanded URLs (not `t.co`)                                     |
| `hashtags`                | JSON    | hashtags without `#`                                           |
| `mentions`                | JSON    | mentioned screen names without `@`                             |
| `media_urls`              | JSON    | image/video URLs                                               |
| `media_types`             | JSON    | `photo` / `video` / `animated_gif`, same order as `media_urls` |

## likes

Primary key is `(account_id, tweet_id)`.

| column       | type | description                      |
| ------------ | ---- | -------------------------------- |
| `account_id` | TEXT | account that liked the tweet     |
| `tweet_id`   | TEXT | liked tweet ID                   |
| `text`       | TEXT | liked tweet's text (may be NULL) |
| `url`        | TEXT | link to the tweet                |

## followers

Primary key is `(account_id, follower_id)`.
The archive contains only user IDs and links, not screen names.

| column        | type | description                  |
| ------------- | ---- | ---------------------------- |
| `account_id`  | TEXT | account being followed       |
| `follower_id` | TEXT | user who follows the account |
| `user_link`   | TEXT | link to the follower         |

## following

Primary key is `(account_id, following_id)`.
The archive contains only user IDs and links, not screen names.

| column         | type | description                  |
| -------------- | ---- | ---------------------------- |
| `account_id`   | TEXT | account following the user   |
| `following_id` | TEXT | user followed by the account |
| `user_link`    | TEXT | link to the followed user    |
