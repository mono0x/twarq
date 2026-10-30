# Database schema

Terminology follows the archive format itself: an _account_ is an archive
owner imported into the database (`data/account.js`), while _user_ refers to
any Twitter user referenced only by ID, such as reply targets and followers.

## accounts

| column         | type    | description              |
| -------------- | ------- | ------------------------ |
| `account_id`   | VARCHAR | account ID (primary key) |
| `username`     | VARCHAR | screen name              |
| `display_name` | VARCHAR | display name             |

## tweets

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

## likes

Primary key is `(account_id, tweet_id)`.

| column       | type    | description                      |
| ------------ | ------- | -------------------------------- |
| `account_id` | VARCHAR | account that liked the tweet     |
| `tweet_id`   | VARCHAR | liked tweet ID                   |
| `text`       | TEXT    | liked tweet's text (may be NULL) |
| `url`        | TEXT    | link to the tweet                |

## followers

Primary key is `(account_id, follower_id)`.
The archive contains only user IDs and links, not screen names.

| column        | type    | description                  |
| ------------- | ------- | ---------------------------- |
| `account_id`  | VARCHAR | account being followed       |
| `follower_id` | VARCHAR | user who follows the account |
| `user_link`   | TEXT    | link to the follower         |

## following

Primary key is `(account_id, following_id)`.
The archive contains only user IDs and links, not screen names.

| column         | type    | description                  |
| -------------- | ------- | ---------------------------- |
| `account_id`   | VARCHAR | account following the user   |
| `following_id` | VARCHAR | user followed by the account |
| `user_link`    | TEXT    | link to the followed user    |
