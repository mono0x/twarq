use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags, params_from_iter};
use serde_json::{Map, Value};

use crate::cli::Format;

/// Opens the database in read-only mode. Every search/query command is a
/// reader; opening read-only means a malformed or hostile query cannot modify
/// the archive, and it allows concurrent readers.
pub fn open_read_only(path: &Path) -> Result<Connection> {
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("failed to open {}", path.display()))
}

/// Executes `sql` and writes the rows to `out` in the requested format.
///
/// `limit`, when given, caps the result so an unbounded SELECT cannot flood
/// the caller (typically an LLM's context window). SQLite steps rows lazily,
/// so stopping early also stops the scan.
pub fn run_query(
    conn: &Connection,
    out: &mut impl Write,
    format: Format,
    sql: &str,
    params: &[String],
    limit: Option<usize>,
) -> Result<()> {
    let mut stmt = conn.prepare(sql)?;
    let columns: Vec<(String, Option<String>)> = stmt
        .columns()
        .map(|c| (c.name().to_string(), c.decl_type().map(str::to_uppercase)))
        .collect();
    let mut rows = stmt.query(params_from_iter(params))?;

    let mut records: Vec<Vec<Value>> = Vec::new();
    while let Some(row) = rows.next()? {
        if limit.is_some_and(|limit| records.len() >= limit) {
            break;
        }
        let record = columns
            .iter()
            .enumerate()
            .map(|(i, (_, decl_type))| Ok(to_json(row.get_ref(i)?, decl_type.as_deref())))
            .collect::<Result<Vec<_>>>()?;
        records.push(record);
    }

    match format {
        Format::Jsonl => {
            for record in records {
                writeln!(out, "{}", Value::Object(to_object(&columns, record)))?;
            }
        }
        Format::Json => {
            let objects: Vec<Value> = records
                .into_iter()
                .map(|record| Value::Object(to_object(&columns, record)))
                .collect();
            serde_json::to_writer_pretty(&mut *out, &objects)?;
            writeln!(out)?;
        }
        Format::Table => {
            let names: Vec<String> = columns.into_iter().map(|(name, _)| name).collect();
            let cells: Vec<Vec<String>> = records
                .iter()
                .map(|record| record.iter().map(to_cell).collect())
                .collect();
            write_table(out, &names, &cells)?;
        }
    }
    Ok(())
}

/// Converts a SQLite value to JSON. SQLite has no list or boolean types, so
/// the column's declared type decides: `JSON` columns (entity lists stored as
/// JSON text) are embedded as JSON, and `BOOLEAN` columns become true/false.
/// The declared type survives subqueries and joins but not expressions, so a
/// computed column falls back to its storage class.
fn to_json(value: ValueRef<'_>, decl_type: Option<&str>) -> Value {
    match (value, decl_type) {
        (ValueRef::Null, _) => Value::Null,
        (ValueRef::Integer(n), Some("BOOLEAN")) => Value::Bool(n != 0),
        (ValueRef::Integer(n), _) => n.into(),
        (ValueRef::Real(f), _) => f.into(),
        (ValueRef::Text(text), Some("JSON")) => serde_json::from_slice(text)
            .unwrap_or_else(|_| String::from_utf8_lossy(text).into_owned().into()),
        (ValueRef::Text(text) | ValueRef::Blob(text), _) => {
            String::from_utf8_lossy(text).into_owned().into()
        }
    }
}

fn to_object(columns: &[(String, Option<String>)], record: Vec<Value>) -> Map<String, Value> {
    columns
        .iter()
        .map(|(name, _)| name.clone())
        .zip(record)
        .collect()
}

fn to_cell(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        // Newlines would break the row alignment.
        Value::String(s) => s.replace('\n', " "),
        other => other.to_string(),
    }
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
        SELECT m.name AS table_name, p.name AS column_name, p.type AS data_type
        FROM sqlite_schema m JOIN pragma_table_info(m.name) p
        WHERE m.type = 'table' AND m.name NOT LIKE 'sqlite\\_%' ESCAPE '\\'
        ORDER BY m.name, p.cid";
    run_query(conn, out, format, COLUMNS, &[], None)?;

    const COUNTS: &str = "
        SELECT 'accounts' AS table_name, count(*) AS rows FROM accounts
        UNION ALL SELECT 'tweets', count(*) FROM tweets
        UNION ALL SELECT 'likes', count(*) FROM likes
        UNION ALL SELECT 'followers', count(*) FROM followers
        UNION ALL SELECT 'following', count(*) FROM following";
    run_query(conn, out, format, COUNTS, &[], None)
}
