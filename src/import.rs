use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use duckdb::{Connection, params};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use zip::ZipArchive;

// No full-text index is built here on purpose. DuckDB's fts extension
// tokenizes on whitespace and punctuation, so Japanese text — which has no
// word spacing — collapses into a single token per tweet and never matches a
// term query. At archive scale a substring scan is a few milliseconds, so
// `search` uses ILIKE instead, which works in every language.
const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS accounts (
        account_id VARCHAR PRIMARY KEY,
        username VARCHAR,
        display_name VARCHAR
    );
    CREATE TABLE IF NOT EXISTS tweets (
        account_id VARCHAR,
        id VARCHAR,
        text TEXT,
        created_at TIMESTAMP,
        retweet_count INTEGER,
        favorite_count INTEGER,
        source TEXT,
        lang VARCHAR,
        deleted BOOLEAN,
        in_reply_to_status_id VARCHAR,
        in_reply_to_user_id VARCHAR,
        in_reply_to_screen_name VARCHAR,
        urls VARCHAR[],
        hashtags VARCHAR[],
        mentions VARCHAR[],
        media_urls VARCHAR[],
        media_types VARCHAR[],
        PRIMARY KEY (account_id, id)
    );
    CREATE TABLE IF NOT EXISTS likes (
        account_id VARCHAR,
        tweet_id VARCHAR,
        text TEXT,
        url TEXT,
        PRIMARY KEY (account_id, tweet_id)
    );
    CREATE TABLE IF NOT EXISTS followers (
        account_id VARCHAR,
        follower_id VARCHAR,
        user_link TEXT,
        PRIMARY KEY (account_id, follower_id)
    );
    CREATE TABLE IF NOT EXISTS following (
        account_id VARCHAR,
        following_id VARCHAR,
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
    let mut zip = ZipArchive::new(BufReader::new(File::open(path)?))?;

    // The account has to be known before any other row can be inserted, but
    // ZIP entry order is arbitrary, so account.js is located in a first pass.
    let mut account: Option<Account> = None;
    for i in 0..zip.len() {
        let file = zip.by_index(i)?;
        if base_name(file.name()) == "account.js" {
            let name = file.name().to_string();
            let mut accounts: Vec<AccountWrapper> = parse_data_file(file, &name)?;
            if let Some(wrapper) = accounts.drain(..).next() {
                account = Some(wrapper.account);
            }
            break;
        }
    }
    let Some(account) = account else {
        bail!("no account.js found; not an X archive?");
    };

    // One transaction per archive: without it every INSERT auto-commits and
    // fsyncs the WAL, which makes a full archive take hours instead of
    // seconds. A statement error poisons a DuckDB transaction (later
    // statements fail, COMMIT silently discards everything), so the inserts
    // below are written to never error on malformed rows — see import_tweets.
    // Rolls back on drop if an error propagates.
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
    for i in 0..zip.len() {
        let file = zip.by_index(i)?;
        let name = file.name().to_string();
        let base = base_name(&name).to_string();
        if is_data_file(&base, "tweets") {
            let tweets: Vec<TweetWrapper> = parse_data_file(file, &name)?;
            import_tweets(&tx, &account, &tweets, false, verbose, &mut stats)?;
            log(verbose, &format!("{name}: {} tweets", tweets.len()));
        } else if is_data_file(&base, "deleted-tweets") {
            let tweets: Vec<TweetWrapper> = parse_data_file(file, &name)?;
            import_tweets(&tx, &account, &tweets, true, verbose, &mut stats)?;
            log(verbose, &format!("{name}: {} deleted tweets", tweets.len()));
        } else if is_data_file(&base, "like") {
            let likes: Vec<LikeWrapper> = parse_data_file(file, &name)?;
            import_likes(&tx, &account, &likes, &mut stats)?;
            log(verbose, &format!("{name}: {} likes", likes.len()));
        } else if is_data_file(&base, "follower") {
            let followers: Vec<FollowerWrapper> = parse_data_file(file, &name)?;
            import_follows(
                &tx,
                &account,
                followers.iter().map(|w| &w.follower),
                "INSERT OR IGNORE INTO followers (account_id, follower_id, user_link)
                 SELECT ?, follow_id, user_link FROM staged_follows",
            )?;
            stats.followers += followers.len();
            log(verbose, &format!("{name}: {} followers", followers.len()));
        } else if is_data_file(&base, "following") {
            let following: Vec<FollowingWrapper> = parse_data_file(file, &name)?;
            import_follows(
                &tx,
                &account,
                following.iter().map(|w| &w.following),
                "INSERT OR IGNORE INTO following (account_id, following_id, user_link)
                 SELECT ?, follow_id, user_link FROM staged_follows",
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

/// Renders a slice as a JSON array destined for a VARCHAR[] column. The
/// driver cannot bind a Rust Vec directly, so lists are staged as JSON text
/// and converted with `from_json(col::JSON, '[\"VARCHAR\"]')` on insert.
fn json_list(values: &[&str]) -> String {
    serde_json::to_string(values).expect("string slices always serialize")
}

fn import_tweets(
    conn: &Connection,
    account: &Account,
    tweets: &[TweetWrapper],
    deleted: bool,
    verbose: bool,
    stats: &mut Stats,
) -> Result<()> {
    // Rows go through a staging table via the appender rather than through a
    // per-row INSERT: executing a statement per tweet is what made imports
    // take hours, while appending and then converting the whole batch in one
    // vectorized INSERT ... SELECT takes seconds.
    conn.execute_batch(
        "CREATE OR REPLACE TEMP TABLE staged_tweets (
            id VARCHAR,
            text VARCHAR,
            created_at VARCHAR,
            retweet_count BIGINT,
            favorite_count BIGINT,
            source VARCHAR,
            lang VARCHAR,
            in_reply_to_status_id VARCHAR,
            in_reply_to_user_id VARCHAR,
            in_reply_to_screen_name VARCHAR,
            urls VARCHAR,
            hashtags VARCHAR,
            mentions VARCHAR,
            media_urls VARCHAR,
            media_types VARCHAR)",
    )?;
    let mut appender = conn.appender_to_catalog_and_db("staged_tweets", "temp", "main")?;
    for wrapper in tweets {
        let tweet = &wrapper.tweet;
        let urls: Vec<&str> = tweet
            .entities
            .urls
            .iter()
            .filter(|u| !u.expanded_url.is_empty())
            .map(|u| u.expanded_url.as_str())
            .collect();
        let hashtags: Vec<&str> = tweet
            .entities
            .hashtags
            .iter()
            .map(|h| h.text.as_str())
            .collect();
        let mentions: Vec<&str> = tweet
            .entities
            .user_mentions
            .iter()
            .map(|m| m.screen_name.as_str())
            .collect();
        let media = &tweet.extended_entities.media;
        let media_urls: Vec<&str> = media.iter().map(|m| m.media_url_https.as_str()).collect();
        let media_types: Vec<&str> = media.iter().map(|m| m.media_type.as_str()).collect();

        appender.append_row(params![
            tweet.id_str,
            tweet.full_text,
            tweet.created_at,
            tweet.retweet_count.parse::<i64>().unwrap_or(0),
            tweet.favorite_count.parse::<i64>().unwrap_or(0),
            tweet.source,
            tweet.lang,
            tweet.in_reply_to_status_id_str,
            tweet.in_reply_to_user_id_str,
            tweet.in_reply_to_screen_name,
            json_list(&urls),
            json_list(&hashtags),
            json_list(&mentions),
            json_list(&media_urls),
            json_list(&media_types),
        ])?;
    }
    appender.flush()?;
    drop(appender);

    // try_strptime instead of strptime, filtered by the WHERE clause: a
    // statement error inside the archive's transaction poisons it in DuckDB —
    // every later statement fails and the commit silently drops everything —
    // so a bad created_at must drop the row without raising an error. The
    // offset is matched literally rather than with %z: archive timestamps are
    // always +0000, and %z would make strptime return TIMESTAMPTZ, whose cast
    // to TIMESTAMP needs the icu extension (not statically linked) and the
    // session timezone.
    conn.execute(
        "INSERT OR IGNORE INTO tweets
           (account_id, id, text, created_at, retweet_count, favorite_count,
            source, lang, deleted, in_reply_to_status_id, in_reply_to_user_id,
            in_reply_to_screen_name, urls, hashtags, mentions, media_urls, media_types)
         SELECT ?, id, text, try_strptime(created_at, '%a %b %d %H:%M:%S +0000 %Y'),
                retweet_count, favorite_count, source, lang, ?,
                in_reply_to_status_id, in_reply_to_user_id, in_reply_to_screen_name,
                from_json(urls::JSON, '[\"VARCHAR\"]'), from_json(hashtags::JSON, '[\"VARCHAR\"]'),
                from_json(mentions::JSON, '[\"VARCHAR\"]'), from_json(media_urls::JSON, '[\"VARCHAR\"]'),
                from_json(media_types::JSON, '[\"VARCHAR\"]')
         FROM staged_tweets
         WHERE try_strptime(created_at, '%a %b %d %H:%M:%S +0000 %Y') IS NOT NULL",
        params![account.account_id, deleted],
    )?;

    let mut skipped_rows = conn.prepare(
        "SELECT id, created_at FROM staged_tweets
         WHERE try_strptime(created_at, '%a %b %d %H:%M:%S +0000 %Y') IS NULL",
    )?;
    let mut skipped = 0usize;
    for row in skipped_rows.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })? {
        let (id, created_at) = row?;
        log(
            verbose,
            &format!("skipping tweet {id}: unparseable created_at {created_at:?}"),
        );
        skipped += 1;
    }
    drop(skipped_rows);
    conn.execute_batch("DROP TABLE staged_tweets")?;

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
    conn.execute_batch(
        "CREATE OR REPLACE TEMP TABLE staged_likes (tweet_id VARCHAR, text VARCHAR, url VARCHAR)",
    )?;
    let mut appender = conn.appender_to_catalog_and_db("staged_likes", "temp", "main")?;
    for wrapper in likes {
        let like = &wrapper.like;
        appender.append_row(params![like.tweet_id, like.full_text, like.expanded_url])?;
    }
    appender.flush()?;
    drop(appender);
    conn.execute(
        "INSERT OR IGNORE INTO likes (account_id, tweet_id, text, url)
         SELECT ?, tweet_id, text, url FROM staged_likes",
        params![account.account_id],
    )?;
    conn.execute_batch("DROP TABLE staged_likes")?;
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
    conn.execute_batch(
        "CREATE OR REPLACE TEMP TABLE staged_follows (follow_id VARCHAR, user_link VARCHAR)",
    )?;
    let mut appender = conn.appender_to_catalog_and_db("staged_follows", "temp", "main")?;
    for follow in follows {
        appender.append_row(params![follow.account_id, follow.user_link])?;
    }
    appender.flush()?;
    drop(appender);
    conn.execute(insert_sql, params![account.account_id])?;
    conn.execute_batch("DROP TABLE staged_follows")?;
    Ok(())
}
