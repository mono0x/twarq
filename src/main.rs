mod cli;
mod import;
mod mcp;
mod query;
mod search;

use std::io::Write;

use anyhow::{Context, Result};
use clap::Parser;

use crate::cli::{Cli, Command};

fn main() {
    let cli = Cli::parse();
    // Not locked: the mcp command writes to stdout from tokio's blocking
    // threads, which would deadlock against a lock held here.
    if let Err(e) = run(cli, &mut std::io::stdout()) {
        eprintln!("Error: {e:#}");
        std::process::exit(1);
    }
}

fn run(cli: Cli, out: &mut impl Write) -> Result<()> {
    match cli.command {
        Command::Import { archives, verbose } => {
            let conn = duckdb::Connection::open(&cli.db)
                .with_context(|| format!("failed to open {}", cli.db.display()))?;
            import::import_archives(&conn, &archives, verbose)
        }
        Command::Search(args) => {
            let conn = query::open_read_only(&cli.db)?;
            search::run(&conn, out, &args)
        }
        Command::Query { sql, limit, format } => {
            let conn = query::open_read_only(&cli.db)?;
            query::run_query(&conn, out, format, &sql, &[], (limit > 0).then_some(limit))
        }
        Command::Mcp { http } => match http {
            Some(addr) => mcp::run_http(&cli.db, addr),
            None => mcp::run_stdio(&cli.db),
        },
        Command::Schema { format } => {
            let conn = query::open_read_only(&cli.db)?;
            query::print_schema(&conn, out, format)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::io::Write;
    use std::path::{Path, PathBuf};

    use clap::Parser;

    use super::*;

    fn make_archive(dir: &Path, name: &str, files: &[(&str, String)]) -> PathBuf {
        let path = dir.join(name);
        let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        for (entry, content) in files {
            zip.start_file(*entry, options).unwrap();
            zip.write_all(content.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
        path
    }

    fn account_js(account_id: &str, username: &str, display_name: &str) -> String {
        format!(
            r#"window.YTD.account.part0 = [ {{ "account": {{ "email": "x@example.com", "createdVia": "web", "username": "{username}", "accountId": "{account_id}", "createdAt": "2010-01-01T00:00:00.000Z", "accountDisplayName": "{display_name}" }} }} ]"#
        )
    }

    fn tweet_js(id: &str, text: &str, created_at: &str, extra: &str) -> String {
        format!(
            r#"{{ "tweet": {{ "id_str": "{id}", "full_text": "{text}", "created_at": "{created_at}", "retweet_count": "1", "favorite_count": "2", "source": "<a href=\"https://example.com\">app</a>", "lang": "ja"{extra} }} }}"#
        )
    }

    fn alice_archive(dir: &Path) -> PathBuf {
        let tweets = format!(
            "window.YTD.tweets.part0 = [ {}, {} ]",
            tweet_js(
                "1001",
                "旅行の写真 #travel",
                "Mon Sep 24 03:35:21 +0000 2018",
                r#", "entities": { "hashtags": [ { "text": "travel" } ], "user_mentions": [], "urls": [ { "expanded_url": "https://example.com/travel" } ] }, "extended_entities": { "media": [ { "media_url_https": "https://pbs.twimg.com/1.jpg", "type": "photo" } ] }"#
            ),
            tweet_js(
                "1002",
                "@carol それな",
                "Tue Sep 25 10:00:00 +0000 2018",
                r#", "in_reply_to_status_id_str": "999", "in_reply_to_user_id_str": "333", "in_reply_to_screen_name": "carol", "entities": { "user_mentions": [ { "screen_name": "carol" } ] }"#
            ),
        );
        let deleted = format!(
            "window.YTD.deleted_tweets.part0 = [ {} ]",
            tweet_js("1003", "消したい過去", "Wed Sep 26 00:00:00 +0000 2018", ""),
        );
        let likes = r#"window.YTD.like.part0 = [
            { "like": { "tweetId": "2001", "fullText": "いいねされた旅行の話", "expandedUrl": "https://x.com/i/web/status/2001" } },
            { "like": { "tweetId": "2002", "fullText": "another liked tweet" } }
        ]"#;
        let follower = r#"window.YTD.follower.part0 = [ { "follower": { "accountId": "501", "userLink": "https://twitter.com/intent/user?user_id=501" } } ]"#;
        let following = r#"window.YTD.following.part0 = [ { "following": { "accountId": "502", "userLink": "https://twitter.com/intent/user?user_id=502" } } ]"#;
        make_archive(
            dir,
            "alice.zip",
            &[
                ("data/account.js", account_js("111", "alice", "Alice")),
                ("data/tweets.js", tweets),
                ("data/deleted-tweets.js", deleted),
                ("data/like.js", likes.to_string()),
                ("data/follower.js", follower.to_string()),
                ("data/following.js", following.to_string()),
            ],
        )
    }

    fn bob_archive(dir: &Path) -> PathBuf {
        // tweets-part1.js exercises the split-file name matching.
        let tweets = format!(
            "window.YTD.tweets.part1 = [ {} ]",
            tweet_js(
                "1101",
                "旅行から帰ってきた",
                "Thu Jan 02 12:00:00 +0000 2020",
                ""
            ),
        );
        let likes = r#"window.YTD.like.part0 = [
            { "like": { "tweetId": "2001", "fullText": "いいねされた旅行の話", "expandedUrl": "https://x.com/i/web/status/2001" } }
        ]"#;
        make_archive(
            dir,
            "bob.zip",
            &[
                ("data/account.js", account_js("222", "bob", "Bob")),
                ("data/tweets-part1.js", tweets),
                ("data/like.js", likes.to_string()),
            ],
        )
    }

    fn run_cli(args: &[&str]) -> String {
        let cli = Cli::try_parse_from(args).unwrap();
        let mut out = Vec::new();
        run(cli, &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    fn setup() -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("test.duckdb").to_str().unwrap().to_string();
        let alice = alice_archive(dir.path());
        let bob = bob_archive(dir.path());
        run_cli(&[
            "twarq",
            "import",
            "--db",
            &db,
            alice.to_str().unwrap(),
            bob.to_str().unwrap(),
        ]);
        (dir, db)
    }

    fn jsonl(output: &str) -> Vec<serde_json::Value> {
        output
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[test]
    fn search_spans_accounts() {
        let (_dir, db) = setup();

        let rows = jsonl(&run_cli(&["twarq", "search", "--db", &db, "旅行"]));
        assert_eq!(rows.len(), 2);
        let usernames: Vec<&str> = rows
            .iter()
            .map(|r| r["username"].as_str().unwrap())
            .collect();
        assert_eq!(usernames, ["bob", "alice"]); // newest first

        let rows = jsonl(&run_cli(&[
            "twarq",
            "search",
            "--db",
            &db,
            "旅行",
            "--account",
            "alice",
        ]));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["id"], "1001");
        assert_eq!(rows[0]["created_at"], "2018-09-24 03:35:21");
        assert_eq!(rows[0]["urls"][0], "https://example.com/travel");
    }

    #[test]
    fn search_filters() {
        let (_dir, db) = setup();

        let rows = jsonl(&run_cli(&[
            "twarq",
            "search",
            "--db",
            &db,
            "--reply-to",
            "@carol",
        ]));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["id"], "1002");

        let rows = jsonl(&run_cli(&["twarq", "search", "--db", &db, "--has-media"]));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["id"], "1001");

        // Deleted tweets are excluded unless asked for.
        let rows = jsonl(&run_cli(&["twarq", "search", "--db", &db, "過去"]));
        assert!(rows.is_empty());
        let rows = jsonl(&run_cli(&[
            "twarq",
            "search",
            "--db",
            &db,
            "--only-deleted",
        ]));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["id"], "1003");

        let rows = jsonl(&run_cli(&[
            "twarq",
            "search",
            "--db",
            &db,
            "--since",
            "2018-09-25",
            "--until",
            "2018-09-25",
        ]));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["id"], "1002");
    }

    #[test]
    fn search_likes() {
        let (_dir, db) = setup();

        // The same tweet liked from two accounts yields one row per account.
        let rows = jsonl(&run_cli(&[
            "twarq", "search", "--db", &db, "--likes", "旅行",
        ]));
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r["tweet_id"] == "2001"));

        let rows = jsonl(&run_cli(&[
            "twarq",
            "search",
            "--db",
            &db,
            "--likes",
            "--account",
            "bob",
        ]));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["url"], "https://x.com/i/web/status/2001");
    }

    #[test]
    fn query_applies_limit_cap() {
        let (_dir, db) = setup();

        let sql = "SELECT id FROM tweets ORDER BY id";
        let rows = jsonl(&run_cli(&["twarq", "query", "--db", &db, sql]));
        assert_eq!(rows.len(), 4);
        let rows = jsonl(&run_cli(&["twarq", "query", "--db", &db, sql, "-n", "1"]));
        assert_eq!(rows.len(), 1);
        let rows = jsonl(&run_cli(&["twarq", "query", "--db", &db, sql, "-n", "0"]));
        assert_eq!(rows.len(), 4);
    }

    #[test]
    fn reimport_adds_no_duplicates() {
        let (dir, db) = setup();

        let alice = dir.path().join("alice.zip");
        run_cli(&["twarq", "import", "--db", &db, alice.to_str().unwrap()]);

        let rows = jsonl(&run_cli(&[
            "twarq",
            "query",
            "--db",
            &db,
            "SELECT count(*) AS n FROM tweets",
        ]));
        assert_eq!(rows[0]["n"], 4);
        let rows = jsonl(&run_cli(&[
            "twarq",
            "query",
            "--db",
            &db,
            "SELECT count(*) AS n FROM likes",
        ]));
        assert_eq!(rows[0]["n"], 3);
    }

    #[test]
    fn import_skips_malformed_rows_within_transaction() {
        // The whole archive imports in one transaction; a row whose
        // created_at fails strptime must be skipped without poisoning the
        // transaction or losing the rows around it.
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("test.duckdb").to_str().unwrap().to_string();
        let tweets = format!(
            "window.YTD.tweets.part0 = [ {}, {} ]",
            tweet_js("1201", "壊れた日付", "Mon Sep 24 03:35:21 +0900 2018", ""),
            tweet_js(
                "1202",
                "正常なツイート",
                "Mon Sep 24 04:00:00 +0000 2018",
                ""
            ),
        );
        let archive = make_archive(
            dir.path(),
            "carol.zip",
            &[
                ("data/account.js", account_js("333", "carol", "Carol")),
                ("data/tweets.js", tweets),
            ],
        );
        run_cli(&["twarq", "import", "--db", &db, archive.to_str().unwrap()]);

        let rows = jsonl(&run_cli(&[
            "twarq",
            "query",
            "--db",
            &db,
            "SELECT id FROM tweets",
        ]));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["id"], "1202");
    }

    #[test]
    fn schema_lists_tables() {
        let (_dir, db) = setup();
        let output = run_cli(&["twarq", "schema", "--db", &db]);
        for table in ["accounts", "tweets", "likes", "followers", "following"] {
            assert!(output.contains(table), "missing {table} in:\n{output}");
        }
    }

    #[test]
    fn table_format_aligns_columns() {
        let (_dir, db) = setup();
        let output = run_cli(&[
            "twarq",
            "query",
            "--db",
            &db,
            "SELECT username FROM accounts ORDER BY username",
            "-f",
            "table",
        ]);
        assert_eq!(output, "username\n--------\nalice\nbob\n");
    }

    #[test]
    fn mcp_serves_tools() {
        use rmcp::ServiceExt;
        use rmcp::model::CallToolRequestParams;

        let (_dir, db) = setup();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let conn = query::open_read_only(Path::new(&db)).unwrap();
            let (client_io, server_io) = tokio::io::duplex(64 * 1024);
            let server = tokio::spawn(async move {
                let service = mcp::McpServer::new(conn).serve(server_io).await.unwrap();
                service.waiting().await.unwrap();
            });
            let client = ().serve(client_io).await.unwrap();

            let tools = client.list_tools(None).await.unwrap();
            let mut names: Vec<&str> = tools.tools.iter().map(|t| t.name.as_ref()).collect();
            names.sort();
            assert_eq!(names, ["query", "schema", "search_likes", "search_tweets"]);

            let mut params = CallToolRequestParams::new("search_tweets");
            params.arguments = serde_json::json!({"keywords": ["旅行"], "account": "alice"})
                .as_object()
                .cloned();
            let result = client.call_tool(params).await.unwrap();
            assert_ne!(result.is_error, Some(true));
            let rows = jsonl(&result.content[0].as_text().unwrap().text);
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0]["id"], "1001");

            // Bad SQL must surface as a tool-level error the model can read,
            // not as an opaque protocol error.
            let mut params = CallToolRequestParams::new("query");
            params.arguments = serde_json::json!({"sql": "SELECT nope"})
                .as_object()
                .cloned();
            let result = client.call_tool(params).await.unwrap();
            assert_eq!(result.is_error, Some(true));

            client.cancel().await.unwrap();
            server.await.unwrap();
        });
    }

    #[test]
    fn mcp_serves_tools_over_http() {
        use axum::body::{Body, to_bytes};
        use axum::http::Request;
        use tower::ServiceExt;

        let (_dir, db) = setup();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let conn = query::open_read_only(Path::new(&db)).unwrap();
            let router = mcp::http_router(mcp::McpServer::new(conn));

            let post = async |body: serde_json::Value| {
                let request = Request::post("/mcp")
                    .header("host", "127.0.0.1")
                    .header("content-type", "application/json")
                    .header("accept", "application/json, text/event-stream")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap();
                let response = router.clone().oneshot(request).await.unwrap();
                assert_eq!(response.status(), 200);
                let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
                serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()
            };

            let response = post(serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": {"name": "test", "version": "0"},
                },
            }))
            .await;
            assert_eq!(response["result"]["serverInfo"]["name"], "twarq");

            let response = post(serde_json::json!({
                "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                "params": {
                    "name": "search_likes",
                    "arguments": {"keywords": ["旅行"], "account": "bob"},
                },
            }))
            .await;
            let rows = jsonl(response["result"]["content"][0]["text"].as_str().unwrap());
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0]["tweet_id"], "2001");
        });
    }

    #[test]
    fn is_data_file_matches_parts() {
        use crate::import::is_data_file;
        assert!(is_data_file("tweets.js", "tweets"));
        assert!(is_data_file("tweets-part1.js", "tweets"));
        assert!(is_data_file("tweets-part12.js", "tweets"));
        assert!(!is_data_file("deleted-tweets.js", "tweets"));
        assert!(!is_data_file("tweets-part.js", "tweets"));
        assert!(!is_data_file("tweetsx.js", "tweets"));
        assert!(is_data_file("deleted-tweets.js", "deleted-tweets"));
    }
}
