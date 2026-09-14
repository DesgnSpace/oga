//! Every read and write the index makes against SQLite. One transaction per
//! rebuild, one prepared statement per shape of write.

use std::collections::HashMap;
use std::path::Path;

use oga_domain::SymbolKind;
use oga_store::{Store, StoreError};
use rusqlite::{OptionalExtension, Row, Transaction, params};

/// The index layout this binary writes. An index built by an older layout is
/// rebuilt rather than read.
pub(crate) const INDEX_SCHEME: u32 = 10;

/// What the index knows about its own last build for one project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexRow {
    pub scheme: u32,
    pub symbol_count: usize,
}

/// One file as the index holds it, before its symbols are loaded.
#[derive(Debug, Clone, PartialEq)]
pub struct FileRow {
    pub id: i64,
    pub path: String,
    pub lang: String,
    pub digest: String,
    pub size: u64,
    pub mtime_ms: i64,
    pub ctime_ms: i64,
    pub lines: u64,
}

/// One symbol as the index holds it.
#[derive(Debug, Clone, PartialEq)]
pub struct SymbolRow {
    pub path: String,
    pub kind: SymbolKind,
    pub name: String,
    pub qualified: String,
    pub parent: Option<String>,
    pub line: u64,
    pub end_line: u64,
    pub signature: String,
    pub doc: Option<String>,
    pub exported: bool,
    pub digest: String,
    pub name_key: String,
    pub tokens: String,
}

/// A file and its symbols, ready to write.
#[derive(Debug, Clone)]
pub struct FileUpdate {
    pub path: String,
    pub lang: String,
    pub digest: String,
    pub size: u64,
    pub mtime_ms: i64,
    pub ctime_ms: i64,
    pub lines: u64,
    pub symbols: Vec<SymbolRow>,
}

const FILE_COLUMNS: &str = "id,path,lang,digest,size,mtime_ms,ctime_ms,lines";
const SYMBOL_COLUMNS: &str =
    "path,kind,name,qualified,parent,line,end_line,signature,doc,exported,digest";

pub fn file_rows(store: &Store, cwd: &Path) -> Result<HashMap<String, FileRow>, StoreError> {
    store.with_connection(|connection| {
        let mut statement = connection.prepare(&format!(
            "SELECT {FILE_COLUMNS} FROM context_files WHERE cwd=? ORDER BY path"
        ))?;
        let rows = statement
            .query_map([cwd.display().to_string()], file_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows
            .into_iter()
            .map(|file| (file.path.clone(), file))
            .collect())
    })
}

/// The indexed files a path names: the file itself, or every file whose path
/// ends with it at a directory boundary, so `query.rs` and `src/query.rs`
/// both resolve. Shortest path first — the least of it someone had to type.
pub fn files_by_path(
    store: &Store,
    cwd: &Path,
    path: &str,
    limit: usize,
) -> Result<Vec<String>, StoreError> {
    let path = path.trim_start_matches("./").trim_start_matches('/');
    if path.is_empty() {
        return Ok(Vec::new());
    }
    let suffix = format!(
        "%/{}",
        path.replace('\\', r"\\")
            .replace('%', r"\%")
            .replace('_', r"\_")
    );
    store.with_connection(|connection| {
        let mut statement = connection.prepare(&format!(
            "SELECT path FROM context_files \
             WHERE cwd=? AND (path=? OR path LIKE ? ESCAPE '\\') \
             ORDER BY length(path) LIMIT {limit}"
        ))?;
        let rows = statement
            .query_map(params![cwd.display().to_string(), path, suffix], |row| {
                row.get(0)
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    })
}

/// Symbols whose exact name matches one of `names`, folded to lookup keys.
pub fn symbols_by_name(
    store: &Store,
    cwd: &Path,
    names: &[String],
    paths: Option<&[String]>,
) -> Result<Vec<SymbolRow>, StoreError> {
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let name_conditions = names
        .iter()
        .map(|_| "(name_key = ? OR instr(' ' || name_key || ' ', ' ' || ? || ' ') > 0)")
        .collect::<Vec<_>>()
        .join(" OR ");
    store.with_connection(|connection| {
        let path_sql = paths
            .filter(|paths| !paths.is_empty())
            .map(|paths| {
                format!(
                    " AND ({})",
                    paths
                        .iter()
                        .map(|_| "path = ? OR path LIKE ? || '/%' ESCAPE '\\'")
                        .collect::<Vec<_>>()
                        .join(" OR ")
                )
            })
            .unwrap_or_default();
        let mut statement = connection.prepare(&format!(
            "SELECT {SYMBOL_COLUMNS} FROM context_symbols WHERE cwd=? AND ({name_conditions}){path_sql} ORDER BY length(name_key), exported DESC, length(qualified)"
        ))?;
        let path_values = paths
            .unwrap_or_default()
            .iter()
            .map(|path| path.trim_end_matches("/**").to_owned())
            .collect::<Vec<_>>();
        let mut arguments: Vec<&dyn rusqlite::ToSql> =
            Vec::with_capacity(names.len() * 2 + 1 + path_values.len() * 2);
        let cwd = cwd.display().to_string();
        arguments.push(&cwd);
        for name in names {
            arguments.push(name);
            arguments.push(name);
        }
        let escaped = path_values
            .iter()
            .map(|path| {
                path
                    .replace('%', "\\%")
                    .replace('_', "\\_")
            })
            .collect::<Vec<_>>();
        for (path, value) in path_values.iter().zip(&escaped) {
            arguments.push(path);
            arguments.push(value);
        }
        let rows = statement
            .query_map(arguments.as_slice(), symbol_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    })
}

/// Symbols the search index matched, best rank first.
pub fn symbols_by_search(
    store: &Store,
    cwd: &Path,
    query: &str,
    limit: usize,
    paths: Option<&[String]>,
) -> Result<Vec<(SymbolRow, f64)>, StoreError> {
    if query.is_empty() {
        return Ok(Vec::new());
    }
    store.with_connection(|connection| {
        let path_sql = paths
            .filter(|paths| !paths.is_empty())
            .map(|paths| {
                format!(
                    " AND ({})",
                    paths
                        .iter()
                        .map(|_| "s.path = ? OR s.path LIKE ? || '/%' ESCAPE '\\'")
                        .collect::<Vec<_>>()
                        .join(" OR ")
                )
            })
            .unwrap_or_default();
        let mut statement = connection.prepare(&format!(
            "SELECT {}, bm25(context_symbols_fts, 0.0, 12.0, 8.0, 10.0, 3.0, 4.0, 2.0) AS rank \
             FROM context_symbols_fts \
             JOIN context_symbols s ON s.id=context_symbols_fts.rowid \
              WHERE context_symbols_fts MATCH ? {path_sql} \
             ORDER BY rank LIMIT {limit}",
            SYMBOL_COLUMNS
                .split(',')
                .map(|column| format!("s.{column}"))
                .collect::<Vec<_>>()
                .join(",")
        ))?;
        let match_query = project_match(cwd, query);
        let mut arguments: Vec<&dyn rusqlite::ToSql> = vec![&match_query];
        let path_values = paths
            .unwrap_or_default()
            .iter()
            .map(|path| path.trim_end_matches("/**").to_owned())
            .collect::<Vec<_>>();
        let escaped = path_values
            .iter()
            .map(|path| path.replace('%', "\\%").replace('_', "\\_"))
            .collect::<Vec<_>>();
        for (path, value) in path_values.iter().zip(&escaped) {
            arguments.push(path);
            arguments.push(value);
        }
        let rows = statement
            .query_map(arguments.as_slice(), |row| {
                Ok((symbol_from_row(row)?, -row.get::<_, f64>(11)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    })
}

/// How many symbols each term reaches. A word that names almost everything
/// says almost nothing about which answer is right.
pub fn term_hits(
    store: &Store,
    cwd: &Path,
    terms: &[String],
) -> Result<HashMap<String, u64>, StoreError> {
    store.with_connection(|connection| {
        let mut statement = connection.prepare(
            "SELECT COUNT(*) FROM context_symbols_fts \
             JOIN context_symbols s ON s.id=context_symbols_fts.rowid \
              WHERE context_symbols_fts MATCH ?",
        )?;
        let mut hits = HashMap::with_capacity(terms.len());
        for term in terms {
            let count: i64 = statement.query_row(
                [project_match(
                    cwd,
                    &format!("\"{}\"", term.replace('"', "\"\"")),
                )],
                |row| row.get(0),
            )?;
            hits.insert(term.clone(), count.max(0) as u64);
        }
        Ok(hits)
    })
}

/// The one symbol in the project whose body still hashes to `digest`, if
/// exactly one does. Two matches mean the body is boilerplate, not identity.
pub fn symbol_by_digest(
    store: &Store,
    cwd: &Path,
    digest: &str,
) -> Result<Option<SymbolRow>, StoreError> {
    store.with_connection(|connection| {
        let mut statement = connection.prepare(&format!(
            "SELECT {SYMBOL_COLUMNS} FROM context_symbols WHERE cwd=? AND digest=? LIMIT 2"
        ))?;
        let rows = statement
            .query_map(params![cwd.display().to_string(), digest], symbol_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok((rows.len() == 1).then(|| rows.into_iter().next().expect("one match")))
    })
}

/// The symbol a written-out `path#name` means: the plain name, or the
/// qualified one, so `TermWeights::of` and `workspace.package.version` land as
/// readily as `of` and `version`. A plain-name match wins over a qualified one.
pub fn symbol_named(
    store: &Store,
    cwd: &Path,
    path: &str,
    name: &str,
) -> Result<Option<SymbolRow>, StoreError> {
    store.with_connection(|connection| {
        Ok(connection
            .query_row(
                &format!(
                    "SELECT {SYMBOL_COLUMNS} FROM context_symbols \
                     WHERE cwd=?1 AND path=?2 AND (name=?3 OR qualified=?3) \
                     ORDER BY name<>?3, line LIMIT 1"
                ),
                params![cwd.display().to_string(), path, name],
                symbol_from_row,
            )
            .optional()?)
    })
}

pub fn symbol_at(
    store: &Store,
    cwd: &Path,
    path: &str,
    name: &str,
) -> Result<Option<SymbolRow>, StoreError> {
    store.with_connection(|connection| {
        Ok(connection
            .query_row(
                &format!(
                    "SELECT {SYMBOL_COLUMNS} FROM context_symbols WHERE cwd=? AND path=? AND name=? ORDER BY line LIMIT 1"
                ),
                params![cwd.display().to_string(), path, name],
                symbol_from_row,
            )
            .optional()?)
    })
}

/// The whole-file hash the index holds for one path, if it holds the file.
pub fn file_digest(store: &Store, cwd: &Path, path: &str) -> Result<Option<String>, StoreError> {
    store.with_connection(|connection| {
        Ok(connection
            .query_row(
                "SELECT digest FROM context_files WHERE cwd=? AND path=? LIMIT 1",
                params![cwd.display().to_string(), path],
                |row| row.get(0),
            )
            .optional()?)
    })
}

pub fn file_exists(store: &Store, cwd: &Path, path: &str) -> Result<bool, StoreError> {
    store.with_connection(|connection| {
        Ok(connection
            .query_row(
                "SELECT 1 FROM context_files WHERE cwd=? AND path=? LIMIT 1",
                params![cwd.display().to_string(), path],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .is_some())
    })
}

/// Write the whole project, dropping whatever the index held for it before.
pub fn replace_files(
    store: &Store,
    cwd: &Path,
    updates: &[FileUpdate],
    now: &str,
) -> Result<(), StoreError> {
    write(store, cwd, updates, &[], true, now)
}

/// Write the files that changed and drop the ones that went, leaving the rest
/// of the index untouched.
pub fn merge_files(
    store: &Store,
    cwd: &Path,
    updates: &[FileUpdate],
    removed: &[String],
    now: &str,
) -> Result<(), StoreError> {
    write(store, cwd, updates, removed, false, now)
}

fn write(
    store: &Store,
    cwd: &Path,
    updates: &[FileUpdate],
    removed: &[String],
    wipe: bool,
    now: &str,
) -> Result<(), StoreError> {
    let cwd = cwd.display().to_string();
    store.transaction(|transaction| {
        if wipe {
            transaction.execute("DELETE FROM context_files WHERE cwd=?", [&cwd])?;
            transaction.execute("DELETE FROM context_symbols WHERE cwd=?", [&cwd])?;
        }
        for path in removed {
            delete_file(transaction, &cwd, path)?;
        }
        let mut upsert_file = transaction.prepare(
            "INSERT INTO context_files(cwd,path,lang,digest,size,mtime_ms,ctime_ms,lines,updated_at) \
             VALUES(?,?,?,?,?,?,?,?,?) \
             ON CONFLICT(cwd,path) DO UPDATE SET \
             lang=excluded.lang,digest=excluded.digest,size=excluded.size,\
             mtime_ms=excluded.mtime_ms,ctime_ms=excluded.ctime_ms,lines=excluded.lines,\
             updated_at=excluded.updated_at \
             RETURNING id",
        )?;
        let mut clear_symbols =
            transaction.prepare("DELETE FROM context_symbols WHERE file_id=?")?;
        let mut insert_symbol = transaction.prepare(
            "INSERT INTO context_symbols(file_id,cwd,path,kind,name,name_key,qualified,parent,\
             line,end_line,signature,doc,exported,digest,tokens) \
             VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
        )?;
        for update in updates {
            let file_id: i64 = upsert_file.query_row(
                params![
                    cwd,
                    update.path,
                    update.lang,
                    update.digest,
                    update.size,
                    update.mtime_ms,
                    update.ctime_ms,
                    update.lines,
                    now
                ],
                |row| row.get(0),
            )?;
            if !wipe {
                clear_symbols.execute([file_id])?;
            }
            for symbol in &update.symbols {
                insert_symbol.execute(params![
                    file_id,
                    cwd,
                    update.path,
                    symbol.kind.as_str(),
                    symbol.name,
                    symbol.name_key,
                    symbol.qualified,
                    symbol.parent.as_deref().unwrap_or_default(),
                    symbol.line,
                    symbol.end_line,
                    symbol.signature,
                    symbol.doc.as_deref().unwrap_or_default(),
                    i64::from(symbol.exported),
                    symbol.digest,
                    symbol.tokens,
                ])?;
            }
        }
        Ok(())
    })
}

/// A file rewritten with the same contents, carrying the stamps it now wears.
pub struct TouchedFile {
    pub path: String,
    pub mtime_ms: i64,
    pub ctime_ms: i64,
}

/// Record that a file was rewritten with the same contents, so the next run
/// takes the cheap path again instead of re-reading it.
pub fn touch_files(
    store: &Store,
    cwd: &Path,
    touched: &[TouchedFile],
    now: &str,
) -> Result<(), StoreError> {
    if touched.is_empty() {
        return Ok(());
    }
    let cwd = cwd.display().to_string();
    store.transaction(|transaction| {
        let mut statement = transaction.prepare(
            "UPDATE context_files SET mtime_ms=?,ctime_ms=?,updated_at=? WHERE cwd=? AND path=?",
        )?;
        for touched in touched {
            statement.execute(params![
                touched.mtime_ms,
                touched.ctime_ms,
                now,
                cwd,
                touched.path
            ])?;
        }
        Ok(())
    })
}

/// Carry a file's rows to its new path without re-reading it.
pub fn move_file(store: &Store, cwd: &Path, from: &str, to: &str) -> Result<(), StoreError> {
    let cwd = cwd.display().to_string();
    store.transaction(|transaction| {
        delete_file(transaction, &cwd, to)?;
        transaction.execute(
            "UPDATE context_files SET path=? WHERE cwd=? AND path=?",
            params![to, cwd, from],
        )?;
        transaction.execute(
            "UPDATE context_symbols SET path=? WHERE cwd=? AND path=?",
            params![to, cwd, from],
        )?;
        Ok(())
    })
}

fn delete_file(transaction: &Transaction<'_>, cwd: &str, path: &str) -> rusqlite::Result<()> {
    transaction.execute(
        "DELETE FROM context_symbols WHERE cwd=? AND path=?",
        params![cwd, path],
    )?;
    transaction.execute(
        "DELETE FROM context_files WHERE cwd=? AND path=?",
        params![cwd, path],
    )?;
    Ok(())
}

pub fn counts(store: &Store, cwd: &Path) -> Result<(usize, usize), StoreError> {
    store.with_connection(|connection| {
        let cwd = cwd.display().to_string();
        let files: i64 = connection.query_row(
            "SELECT COUNT(*) FROM context_files WHERE cwd=?",
            [&cwd],
            |row| row.get(0),
        )?;
        let symbols: i64 = connection.query_row(
            "SELECT COUNT(*) FROM context_symbols WHERE cwd=?",
            [&cwd],
            |row| row.get(0),
        )?;
        Ok((files.max(0) as usize, symbols.max(0) as usize))
    })
}

pub fn index_row(store: &Store, cwd: &Path) -> Result<Option<IndexRow>, StoreError> {
    store.with_connection(|connection| {
        Ok(connection
            .query_row(
                "SELECT scheme, symbol_count FROM context_index WHERE cwd=?",
                [cwd.display().to_string()],
                |row| {
                    Ok(IndexRow {
                        scheme: row.get::<_, i64>(0)?.max(0) as u32,
                        symbol_count: row.get::<_, i64>(1)?.max(0) as usize,
                    })
                },
            )
            .optional()?)
    })
}

fn project_match(cwd: &Path, query: &str) -> String {
    format!(
        "{{cwd}} : \"{}\" AND ({query})",
        cwd.display().to_string().replace('"', "\"\"")
    )
}

pub fn save_index(
    store: &Store,
    cwd: &Path,
    partial: bool,
    file_count: usize,
    symbol_count: usize,
    now: &str,
) -> Result<(), StoreError> {
    let state = if partial { "partial" } else { "ready" };
    store.transaction(|transaction| {
        transaction.execute(
            "INSERT INTO context_index(cwd,scheme,state,built_at,file_count,symbol_count,updated_at) \
             VALUES(?,?,?,?,?,?,?) \
             ON CONFLICT(cwd) DO UPDATE SET scheme=excluded.scheme,state=excluded.state,\
             built_at=excluded.built_at,file_count=excluded.file_count,\
             symbol_count=excluded.symbol_count,updated_at=excluded.updated_at",
            params![
                cwd.display().to_string(),
                INDEX_SCHEME,
                state,
                now,
                file_count,
                symbol_count,
                now
            ],
        )?;
        Ok(())
    })
}

fn file_from_row(row: &Row<'_>) -> rusqlite::Result<FileRow> {
    Ok(FileRow {
        id: row.get(0)?,
        path: row.get(1)?,
        lang: row.get(2)?,
        digest: row.get(3)?,
        size: row.get::<_, i64>(4)?.max(0) as u64,
        mtime_ms: row.get(5)?,
        ctime_ms: row.get(6)?,
        lines: row.get::<_, i64>(7)?.max(0) as u64,
    })
}

fn symbol_from_row(row: &Row<'_>) -> rusqlite::Result<SymbolRow> {
    Ok(SymbolRow {
        path: row.get(0)?,
        kind: SymbolKind::parse(&row.get::<_, String>(1)?).unwrap_or(SymbolKind::Fn),
        name: row.get(2)?,
        qualified: row.get(3)?,
        parent: non_empty(row.get(4)?),
        line: row.get::<_, i64>(5)?.max(0) as u64,
        end_line: row.get::<_, i64>(6)?.max(0) as u64,
        signature: row.get(7)?,
        doc: non_empty(row.get(8)?),
        exported: row.get::<_, i64>(9)? != 0,
        digest: row.get(10)?,
        name_key: String::new(),
        tokens: String::new(),
    })
}

fn non_empty(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}
