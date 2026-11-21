use std::borrow::Cow;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, params};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use zip::ZipArchive;
use zip::read::{ArchiveOffset, Config};

// No full-text index is built here on purpose. SQLite's FTS5 default
// tokenizer splits on whitespace and punctuation, so Japanese text — which has
// no word spacing — collapses into a single token per sentence and never
// matches a term query. At archive scale a substring scan is a few
// milliseconds, so `search` uses LIKE instead, which works in every language.
//
// SQLite has no list or boolean types. Entity lists are stored as JSON text
// in columns declared `JSON`, and flags as 0/1 in columns declared `BOOLEAN`;
// `query::to_json` relies on these declared types to emit real JSON arrays
// and booleans.
const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS accounts (
        account_id TEXT PRIMARY KEY,
        username TEXT,
        display_name TEXT
    );
    CREATE TABLE IF NOT EXISTS tweets (
        account_id TEXT,
        id TEXT,
        text TEXT,
        created_at TEXT,
        retweet_count INTEGER,
        favorite_count INTEGER,
        source TEXT,
        lang TEXT,
        deleted BOOLEAN,
        in_reply_to_status_id TEXT,
        in_reply_to_user_id TEXT,
        in_reply_to_screen_name TEXT,
        urls JSON,
        hashtags JSON,
        mentions JSON,
        media_urls JSON,
        media_types JSON,
        PRIMARY KEY (account_id, id)
    );
    CREATE TABLE IF NOT EXISTS likes (
        account_id TEXT,
        tweet_id TEXT,
        text TEXT,
        url TEXT,
        PRIMARY KEY (account_id, tweet_id)
    );
    CREATE TABLE IF NOT EXISTS followers (
        account_id TEXT,
        follower_id TEXT,
        user_link TEXT,
        PRIMARY KEY (account_id, follower_id)
    );
    CREATE TABLE IF NOT EXISTS following (
        account_id TEXT,
        following_id TEXT,
        user_link TEXT,
        PRIMARY KEY (account_id, following_id)
    );";

pub fn import_archives(conn: &Connection, archives: &[PathBuf], verbose: bool) -> Result<()> {
    conn.execute_batch(SCHEMA)?;
    for path in archives {
        import_archive(conn, path, verbose)
            .with_context(|| format!("failed to import {}", path.display()))?;
    }
    Ok(())
}

#[derive(Deserialize)]
struct AccountWrapper {
    account: Account,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Account {
    account_id: String,
    username: String,
    account_display_name: String,
}

#[derive(Deserialize)]
struct TweetWrapper {
    tweet: Tweet,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Tweet {
    id_str: String,
    full_text: String,
    created_at: String,
    retweet_count: String,
    favorite_count: String,
    source: String,
    lang: String,
    in_reply_to_status_id_str: Option<String>,
    in_reply_to_user_id_str: Option<String>,
    in_reply_to_screen_name: Option<String>,
    entities: Entities,
    // Multi-photo tweets list every image only under extended_entities;
    // entities.media holds just the first one.
    extended_entities: ExtendedEntities,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Entities {
    hashtags: Vec<Hashtag>,
    user_mentions: Vec<UserMention>,
    urls: Vec<Url>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Hashtag {
    text: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct UserMention {
    screen_name: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Url {
    expanded_url: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ExtendedEntities {
    media: Vec<Media>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Media {
    media_url_https: String,
    #[serde(rename = "type")]
    media_type: String,
}

#[derive(Deserialize)]
struct LikeWrapper {
    like: Like,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Like {
    tweet_id: String,
    full_text: Option<String>,
    expanded_url: Option<String>,
}

#[derive(Deserialize)]
struct FollowerWrapper {
    follower: Follow,
}

#[derive(Deserialize)]
struct FollowingWrapper {
    following: Follow,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Follow {
    account_id: String,
    user_link: String,
}

fn base_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Matches the archive's data file names. X splits large files into numbered
/// parts (tweets.js, tweets-part1.js, ...), so both forms are accepted; base
/// names are compared in full so that deleted-tweets.js is never mistaken for
/// tweets.js.
pub(crate) fn is_data_file(name: &str, base: &str) -> bool {
    let Some(rest) = name.strip_prefix(base) else {
        return false;
    };
    match rest.strip_prefix("-part") {
        None => rest == ".js",
        Some(digits) => digits
            .strip_suffix(".js")
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())),
    }
}

/// Reads one of the archive's .js data files. They are not JSON: each starts
/// with a `window.YTD.<name>.partN = ` assignment followed by a JSON array, so
/// the prefix is stripped before deserializing.
fn parse_data_file<T: DeserializeOwned>(mut file: impl Read, name: &str) -> Result<Vec<T>> {
    let mut content = String::new();
    file.read_to_string(&mut content)?;
    let json_start = content
        .find('[')
        .with_context(|| format!("invalid {name} format: no JSON array found"))?;
    serde_json::from_str(&content[json_start..]).with_context(|| format!("failed to parse {name}"))
}

fn import_archive(conn: &Connection, path: &Path, verbose: bool) -> Result<()> {
    // X archives start at offset 0. The default ArchiveOffset::Detect scans a
    // ZIP64 archive (over 4 GB, which media makes common) from the start for
    // the EOCD64 signature, reading the whole file.
    let config = Config {
        archive_offset: ArchiveOffset::Known(0),
    };
    let mut zip = ZipArchive::with_config(config, BufReader::new(File::open(path)?))?;

    // Entry names come from the central directory, which ZipArchive::with_config
    // has already read. by_index additionally reads the entry's local header, a
    // seek into the archive per call; archives hold thousands of media files
    // and can be several GB on slow disks, so only data files are opened.
    let names: Vec<String> = zip
        .file_names()
        .map(|name| name.map(Cow::into_owned))
        .collect::<Result<_, _>>()?;

    // The account has to be known before any other row can be inserted, but
    // ZIP entry order is arbitrary, so account.js is located first.
    let account = match names.iter().position(|n| base_name(n) == "account.js") {
        Some(i) => parse_data_file::<AccountWrapper>(zip.by_index(i)?, &names[i])?
            .into_iter()
            .next()
            .map(|wrapper| wrapper.account),
        None => None,
    };
    let Some(account) = account else {
        bail!("no account.js found; not an X archive?");
    };

    // One transaction per archive: without it every INSERT auto-commits and
    // syncs the journal, which makes a full archive take minutes instead of
    // seconds. Rolls back on drop if an error propagates, so an archive is
    // imported entirely or not at all.
    let tx = conn.unchecked_transaction()?;

    // OR REPLACE rather than OR IGNORE: a newer archive of the same account
    // should win, picking up renames.
    tx.execute(
        "INSERT OR REPLACE INTO accounts (account_id, username, display_name) VALUES (?, ?, ?)",
        params![
            account.account_id,
            account.username,
            account.account_display_name
        ],
    )?;

    let mut stats = Stats::default();
    for (i, name) in names.iter().enumerate() {
        let base = base_name(name);
        if is_data_file(base, "tweets") {
            let tweets: Vec<TweetWrapper> = parse_data_file(zip.by_index(i)?, name)?;
            import_tweets(&tx, &account, &tweets, false, verbose, &mut stats)?;
            log(verbose, &format!("{name}: {} tweets", tweets.len()));
        } else if is_data_file(base, "deleted-tweets") {
            let tweets: Vec<TweetWrapper> = parse_data_file(zip.by_index(i)?, name)?;
            import_tweets(&tx, &account, &tweets, true, verbose, &mut stats)?;
            log(verbose, &format!("{name}: {} deleted tweets", tweets.len()));
        } else if is_data_file(base, "like") {
            let likes: Vec<LikeWrapper> = parse_data_file(zip.by_index(i)?, name)?;
            import_likes(&tx, &account, &likes, &mut stats)?;
            log(verbose, &format!("{name}: {} likes", likes.len()));
        } else if is_data_file(base, "follower") {
            let followers: Vec<FollowerWrapper> = parse_data_file(zip.by_index(i)?, name)?;
            import_follows(
                &tx,
                &account,
                followers.iter().map(|w| &w.follower),
                "INSERT OR IGNORE INTO followers (account_id, follower_id, user_link)
                 VALUES (?, ?, ?)",
            )?;
            stats.followers += followers.len();
            log(verbose, &format!("{name}: {} followers", followers.len()));
        } else if is_data_file(base, "following") {
            let following: Vec<FollowingWrapper> = parse_data_file(zip.by_index(i)?, name)?;
            import_follows(
                &tx,
                &account,
                following.iter().map(|w| &w.following),
                "INSERT OR IGNORE INTO following (account_id, following_id, user_link)
                 VALUES (?, ?, ?)",
            )?;
            stats.following += following.len();
            log(verbose, &format!("{name}: {} following", following.len()));
        }
    }
    tx.commit()?;

    println!(
        "@{}: imported {} tweets ({} deleted), {} likes, {} followers, {} following",
        account.username,
        stats.tweets,
        stats.deleted_tweets,
        stats.likes,
        stats.followers,
        stats.following
    );
    if stats.skipped > 0 {
        eprintln!(
            "Warning: skipped {} rows (run with --verbose for details)",
            stats.skipped
        );
    }
    Ok(())
}

#[derive(Default)]
struct Stats {
    tweets: usize,
    deleted_tweets: usize,
    likes: usize,
    followers: usize,
    following: usize,
    skipped: usize,
}

fn log(verbose: bool, message: &str) {
    if verbose {
        eprintln!("{message}");
    }
}

/// Converts the archive's `created_at` (e.g. `Mon Sep 24 03:35:21 +0000 2018`)
/// to `2018-09-24 03:35:21`, the form SQLite's date functions understand and
/// that sorts chronologically as text. Archive timestamps are always UTC, so
/// any other offset is rejected rather than converted.
fn parse_created_at(value: &str) -> Option<String> {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let [_, month, day, time, "+0000", year] = value.split(' ').collect::<Vec<_>>()[..] else {
        return None;
    };
    let month = MONTHS.iter().position(|&m| m == month)? + 1;
    let day: u8 = day.parse().ok().filter(|d| (1..=31).contains(d))?;
    let year: u16 = year.parse().ok().filter(|y| (1000..=9999).contains(y))?;
    let [hour, minute, second] = time.split(':').collect::<Vec<_>>()[..] else {
        return None;
    };
    let hour: u8 = hour.parse().ok().filter(|h| *h < 24)?;
    let minute: u8 = minute.parse().ok().filter(|m| *m < 60)?;
    let second: u8 = second.parse().ok().filter(|s| *s < 61)?;
    Some(format!(
        "{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}"
    ))
}

fn json_list<'a>(values: impl IntoIterator<Item = &'a str>) -> String {
    serde_json::to_string(&values.into_iter().collect::<Vec<_>>())
        .expect("string slices always serialize")
}

fn import_tweets(
    conn: &Connection,
    account: &Account,
    tweets: &[TweetWrapper],
    deleted: bool,
    verbose: bool,
    stats: &mut Stats,
) -> Result<()> {
    let mut insert = conn.prepare(
        "INSERT OR IGNORE INTO tweets
           (account_id, id, text, created_at, retweet_count, favorite_count,
            source, lang, deleted, in_reply_to_status_id, in_reply_to_user_id,
            in_reply_to_screen_name, urls, hashtags, mentions, media_urls, media_types)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )?;
    let mut skipped = 0usize;
    for wrapper in tweets {
        let tweet = &wrapper.tweet;
        let Some(created_at) = parse_created_at(&tweet.created_at) else {
            log(
                verbose,
                &format!(
                    "skipping tweet {}: unparseable created_at {:?}",
                    tweet.id_str, tweet.created_at
                ),
            );
            skipped += 1;
            continue;
        };
        let entities = &tweet.entities;
        let media = &tweet.extended_entities.media;
        insert.execute(params![
            account.account_id,
            tweet.id_str,
            tweet.full_text,
            created_at,
            tweet.retweet_count.parse::<i64>().unwrap_or(0),
            tweet.favorite_count.parse::<i64>().unwrap_or(0),
            tweet.source,
            tweet.lang,
            deleted,
            tweet.in_reply_to_status_id_str,
            tweet.in_reply_to_user_id_str,
            tweet.in_reply_to_screen_name,
            json_list(
                entities
                    .urls
                    .iter()
                    .map(|u| u.expanded_url.as_str())
                    .filter(|u| !u.is_empty())
            ),
            json_list(entities.hashtags.iter().map(|h| h.text.as_str())),
            json_list(
                entities
                    .user_mentions
                    .iter()
                    .map(|m| m.screen_name.as_str())
            ),
            json_list(media.iter().map(|m| m.media_url_https.as_str())),
            json_list(media.iter().map(|m| m.media_type.as_str())),
        ])?;
    }

    stats.skipped += skipped;
    if deleted {
        stats.deleted_tweets += tweets.len() - skipped;
    } else {
        stats.tweets += tweets.len() - skipped;
    }
    Ok(())
}

fn import_likes(
    conn: &Connection,
    account: &Account,
    likes: &[LikeWrapper],
    stats: &mut Stats,
) -> Result<()> {
    let mut insert = conn.prepare(
        "INSERT OR IGNORE INTO likes (account_id, tweet_id, text, url) VALUES (?, ?, ?, ?)",
    )?;
    for wrapper in likes {
        let like = &wrapper.like;
        insert.execute(params![
            account.account_id,
            like.tweet_id,
            like.full_text,
            like.expanded_url
        ])?;
    }
    stats.likes += likes.len();
    Ok(())
}

/// Imports followers or following; the two differ only in the target table.
fn import_follows<'a>(
    conn: &Connection,
    account: &Account,
    follows: impl IntoIterator<Item = &'a Follow>,
    insert_sql: &str,
) -> Result<()> {
    let mut insert = conn.prepare(insert_sql)?;
    for follow in follows {
        insert.execute(params![
            account.account_id,
            follow.account_id,
            follow.user_link
        ])?;
    }
    Ok(())
}
