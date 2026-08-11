use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};
use duckdb::{AccessMode, Config, Connection, params_from_iter};

use crate::cli::Format;

/// Opens the database in read-only mode. Every search/query command is a
/// reader; opening read-only means a malformed or hostile query cannot modify
/// the archive, and it allows concurrent readers.
pub fn open_read_only(path: &Path) -> Result<Connection> {
    let config = Config::default().access_mode(AccessMode::ReadOnly)?;
    Connection::open_with_flags(path, config)
        .with_context(|| format!("failed to open {}", path.display()))
}

/// Executes `sql` and writes the rows to `out` in the requested format.
///
/// The statement is wrapped in an outer query that makes DuckDB itself do the
/// serialization: `to_json` for jsonl/json (which handles lists, timestamps
/// and NULLs correctly), and a `COLUMNS(*)::VARCHAR` cast for table output.
/// `limit`, when given, caps the result on the outer query so an unbounded
/// SELECT cannot flood the caller (typically an LLM's context window).
pub fn run_query(
    conn: &Connection,
    out: &mut impl Write,
    format: Format,
    sql: &str,
    params: &[String],
    limit: Option<usize>,
) -> Result<()> {
    let sql = sql.trim().trim_end_matches(';');
    let projection = match format {
        Format::Jsonl | Format::Json => "to_json(t)::VARCHAR",
        Format::Table => "COLUMNS(*)::VARCHAR",
    };
    let mut wrapped = format!("SELECT {projection} FROM ({sql}) t");
    if let Some(limit) = limit {
        wrapped.push_str(&format!(" LIMIT {limit}"));
    }

    let mut stmt = conn.prepare(&wrapped)?;
    let mut rows = stmt.query(params_from_iter(params))?;

    match format {
        Format::Jsonl => {
            while let Some(row) = rows.next()? {
                let json: String = row.get(0)?;
                writeln!(out, "{json}")?;
            }
        }
        Format::Json => {
            let mut records = Vec::new();
            while let Some(row) = rows.next()? {
                let json: String = row.get(0)?;
                records.push(serde_json::from_str::<serde_json::Value>(&json)?);
            }
            serde_json::to_writer_pretty(&mut *out, &records)?;
            writeln!(out)?;
        }
        Format::Table => {
            let columns = rows
                .as_ref()
                .expect("statement is live while rows are read")
                .column_names();
            let mut cells: Vec<Vec<String>> = Vec::new();
            while let Some(row) = rows.next()? {
                let record = (0..columns.len())
                    .map(|i| {
                        let value: Option<String> = row.get(i)?;
                        // Newlines would break the row alignment.
                        Ok(value.unwrap_or_default().replace('\n', " "))
                    })
                    .collect::<duckdb::Result<Vec<_>>>()?;
                cells.push(record);
            }
            write_table(out, &columns, &cells)?;
        }
    }
    Ok(())
}

fn write_table(out: &mut impl Write, columns: &[String], cells: &[Vec<String>]) -> Result<()> {
    let widths: Vec<usize> = columns
        .iter()
        .enumerate()
        .map(|(i, column)| {
            cells
                .iter()
                .map(|record| record[i].chars().count())
                .chain([column.chars().count()])
                .max()
                .unwrap_or(0)
        })
        .collect();

    let write_row = |out: &mut dyn Write, values: &[String]| -> Result<()> {
        let row = values
            .iter()
            .zip(&widths)
            .map(|(value, width)| format!("{value:<width$}"))
            .collect::<Vec<_>>()
            .join("  ");
        writeln!(out, "{}", row.trim_end())?;
        Ok(())
    };

    write_row(out, columns)?;
    let separators: Vec<String> = widths.iter().map(|w| "-".repeat(*w)).collect();
    write_row(out, &separators)?;
    for record in cells {
        write_row(out, record)?;
    }
    Ok(())
}

pub fn print_schema(conn: &Connection, out: &mut impl Write, format: Format) -> Result<()> {
    const COLUMNS: &str = "
        SELECT table_name, column_name, data_type
        FROM information_schema.columns
        WHERE table_schema = 'main'
        ORDER BY table_name, ordinal_position";
    run_query(conn, out, format, COLUMNS, &[], None)?;

    const COUNTS: &str = "
        SELECT 'accounts' AS table_name, count(*) AS rows FROM accounts
        UNION ALL SELECT 'tweets', count(*) FROM tweets
        UNION ALL SELECT 'likes', count(*) FROM likes
        UNION ALL SELECT 'followers', count(*) FROM followers
        UNION ALL SELECT 'following', count(*) FROM following";
    run_query(conn, out, format, COUNTS, &[], None)
}
