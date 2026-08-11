use std::io::Write;

use anyhow::Result;
use duckdb::Connection;

use crate::cli::SearchArgs;
use crate::query::run_query;

pub fn run(conn: &Connection, out: &mut impl Write, args: &SearchArgs) -> Result<()> {
    let mut conditions: Vec<String> = Vec::new();
    let mut params: Vec<String> = Vec::new();

    let text_column = if args.likes { "l.text" } else { "t.text" };
    for keyword in &args.keywords {
        conditions.push(format!("{text_column} ILIKE '%' || ? || '%'"));
        params.push(keyword.clone());
    }
    if let Some(account) = &args.account {
        conditions.push("a.username = ?".into());
        params.push(account.trim_start_matches('@').to_string());
    }

    if args.likes {
        return search_likes(conn, out, args, conditions, params);
    }

    if let Some(since) = &args.since {
        conditions.push("t.created_at >= ?::TIMESTAMP".into());
        params.push(since.clone());
    }
    if let Some(until) = &args.until {
        // Inclusive of the whole end day when a bare date is given.
        conditions.push("t.created_at < ?::TIMESTAMP + INTERVAL 1 DAY".into());
        params.push(until.clone());
    }
    if let Some(mentions) = &args.mentions {
        conditions.push("list_contains(t.mentions, ?)".into());
        params.push(mentions.trim_start_matches('@').to_string());
    }
    if let Some(reply_to) = &args.reply_to {
        conditions.push("t.in_reply_to_screen_name = ?".into());
        params.push(reply_to.trim_start_matches('@').to_string());
    }
    if args.has_media {
        conditions.push("len(t.media_urls) > 0".into());
    }
    if args.has_url {
        conditions.push("len(t.urls) > 0".into());
    }
    if args.no_retweets {
        conditions.push("t.text NOT LIKE 'RT @%'".into());
    }
    if args.only_deleted {
        conditions.push("t.deleted".into());
    } else if !args.include_deleted {
        conditions.push("NOT t.deleted".into());
    }

    let sql = format!(
        "SELECT t.id, a.username, t.created_at, t.text, t.in_reply_to_screen_name,
                t.favorite_count, t.retweet_count, t.urls, t.media_urls, t.deleted
         FROM tweets t JOIN accounts a USING (account_id)
         WHERE {}
         ORDER BY t.created_at {}
         LIMIT {}",
        where_clause(&conditions),
        if args.oldest_first { "ASC" } else { "DESC" },
        args.limit,
    );
    run_query(conn, out, args.format, &sql, &params, None)
}

fn search_likes(
    conn: &Connection,
    out: &mut impl Write,
    args: &SearchArgs,
    conditions: Vec<String>,
    params: Vec<String>,
) -> Result<()> {
    // Likes carry no timestamp in the archive; tweet IDs are snowflakes, so
    // sorting by their numeric value approximates recency.
    let sql = format!(
        "SELECT l.tweet_id, a.username, l.text, l.url
         FROM likes l JOIN accounts a USING (account_id)
         WHERE {}
         ORDER BY try_cast(l.tweet_id AS UBIGINT) {} NULLS LAST
         LIMIT {}",
        where_clause(&conditions),
        if args.oldest_first { "ASC" } else { "DESC" },
        args.limit,
    );
    run_query(conn, out, args.format, &sql, &params, None)
}

fn where_clause(conditions: &[String]) -> String {
    if conditions.is_empty() {
        "TRUE".to_string()
    } else {
        conditions.join(" AND ")
    }
}
