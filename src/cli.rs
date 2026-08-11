use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(version, about = "Import and search X (Twitter) archives with DuckDB")]
pub struct Cli {
    /// DuckDB file path
    #[arg(short, long, global = true, default_value = "twarq.duckdb")]
    pub db: PathBuf,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Import X archive ZIP files into DuckDB
    ///
    /// Each archive is keyed by the account it belongs to (data/account.js),
    /// so archives from multiple accounts can be imported into the same
    /// database. Re-importing the same archive adds no duplicates.
    Import {
        /// Paths to X archive ZIP files
        #[arg(required = true)]
        archives: Vec<PathBuf>,

        /// Print per-file progress to stderr
        #[arg(short, long)]
        verbose: bool,
    },

    /// Search tweets or likes by keyword and filters
    ///
    /// Multiple keywords are ANDed and matched case-insensitively as
    /// substrings, which works for Japanese as well as space-delimited
    /// languages. Deleted tweets are excluded by default.
    Search(SearchArgs),

    /// Run a read-only SQL query
    ///
    /// The database is opened read-only, so statements that would modify it
    /// are rejected by DuckDB itself. Results are capped at --limit rows.
    /// Run `twarq schema` first to see the available tables and columns.
    Query {
        /// SQL to run (SELECT form)
        sql: String,

        /// Maximum rows to return (0 disables the cap)
        #[arg(short = 'n', long, default_value_t = 100)]
        limit: usize,

        #[arg(short, long, value_enum, default_value_t = Format::Jsonl)]
        format: Format,
    },

    /// Show the tables, columns and row counts available for querying
    Schema {
        #[arg(short, long, value_enum, default_value_t = Format::Table)]
        format: Format,
    },
}

#[derive(Args)]
pub struct SearchArgs {
    /// Keywords (ANDed, case-insensitive substring match)
    pub keywords: Vec<String>,

    /// Only tweets/likes of the account with this username
    #[arg(long)]
    pub account: Option<String>,

    /// Search likes instead of tweets (keywords match the liked tweet's text)
    #[arg(long, conflicts_with_all = [
        "since", "until", "mentions", "reply_to", "has_media", "has_url",
        "no_retweets", "include_deleted", "only_deleted",
    ])]
    pub likes: bool,

    /// Only tweets on or after this date (YYYY-MM-DD)
    #[arg(long)]
    pub since: Option<String>,

    /// Only tweets on or before this date (YYYY-MM-DD)
    #[arg(long)]
    pub until: Option<String>,

    /// Only tweets mentioning this screen name
    #[arg(long)]
    pub mentions: Option<String>,

    /// Only replies to this screen name
    #[arg(long)]
    pub reply_to: Option<String>,

    /// Only tweets with attached media
    #[arg(long)]
    pub has_media: bool,

    /// Only tweets containing a link
    #[arg(long)]
    pub has_url: bool,

    /// Exclude retweets
    #[arg(long)]
    pub no_retweets: bool,

    /// Include deleted tweets
    #[arg(long, conflicts_with = "only_deleted")]
    pub include_deleted: bool,

    /// Show only deleted tweets
    #[arg(long)]
    pub only_deleted: bool,

    /// Sort oldest first (default newest first)
    #[arg(long)]
    pub oldest_first: bool,

    /// Maximum rows to return
    #[arg(short = 'n', long, default_value_t = 20)]
    pub limit: usize,

    #[arg(short, long, value_enum, default_value_t = Format::Jsonl)]
    pub format: Format,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Format {
    Jsonl,
    Json,
    Table,
}
