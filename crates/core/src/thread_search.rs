//! Project-local, rebuildable index of chat titles and visible message text.
//! Refresh on search so streaming saves do not rewrite a second copy of the chat.

use std::collections::HashMap;
use std::fs::{self, File, Metadata};
use std::io::BufReader;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{
    params, Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior,
};
use serde::Deserialize;

use crate::thread::{match_excerpt, ThreadId, ThreadStore, THREAD_FORMAT_VERSION};
use crate::{CancelToken, HarnessError};

pub const SEARCH_LIMIT: usize = 24;
const DATABASE: &str = "chat-search-v1.sqlite3";
type IndexResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug)]
pub struct ChatMatch {
    pub thread_id: String,
    pub title: String,
    pub updated_at: u64,
    pub message_id: Option<String>,
    pub excerpt: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Document {
    #[serde(default)]
    version: u32,
    id: String,
    title: Option<String>,
    #[serde(default)]
    updated_at: u64,
    #[serde(default)]
    messages: Vec<Line>,
}

#[derive(Deserialize)]
struct Line {
    id: String,
    role: String,
    #[serde(default)]
    text: String,
    // Older Tauri transcripts could represent reasoning as its own visible row.
    thinking: Option<String>,
}

impl ThreadStore {
    /// Return one hit per chat, preferring a message anchor over a title match.
    /// GPUI already lists title matches separately, so it requests messages only.
    pub fn search_chats(
        &self,
        query: &str,
        include_titles: bool,
        cancel: &CancelToken,
    ) -> crate::Result<Vec<ChatMatch>> {
        let query = query.trim();
        if query.is_empty() || cancel.is_cancelled() {
            return Ok(Vec::new());
        }
        let run = |mut db: Connection| -> IndexResult<_> {
            initialize(&db)?;
            refresh(self, &mut db, cancel)?;
            if cancel.is_cancelled() {
                return Ok(Vec::new());
            }
            find(&db, query, include_titles)
        };
        // A read-only checkout or an unusable derived index must not hide the
        // authoritative transcripts. In those cases search an in-memory index.
        Connection::open(self.dir().join(DATABASE))
            .map_err(Into::into)
            .and_then(&run)
            .or_else(|_: Box<dyn std::error::Error + Send + Sync>| {
                run(Connection::open_in_memory()?)
            })
            .map_err(|e| HarnessError::Other(format!("search chats: {e}")))
    }
}

fn initialize(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch("PRAGMA secure_delete=ON")?;
    if db.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))? == 1 {
        return Ok(());
    }
    db.execute_batch(
        "BEGIN IMMEDIATE;
         CREATE TABLE IF NOT EXISTS chat (
             id TEXT PRIMARY KEY, title TEXT NOT NULL, updated INTEGER NOT NULL,
             modified INTEGER NOT NULL, created INTEGER NOT NULL, bytes INTEGER NOT NULL,
             first_row INTEGER NOT NULL, last_row INTEGER NOT NULL
         );
         CREATE VIRTUAL TABLE IF NOT EXISTS line USING fts5(
             body, thread UNINDEXED, message UNINDEXED, tokenize='trigram'
         );
         INSERT INTO line(line, rank) VALUES('secure-delete', 1);
         PRAGMA user_version=1;
         COMMIT;",
    )?;
    Ok(())
}

type Stamp = (i64, i64, i64);

fn stamp(metadata: &Metadata) -> Stamp {
    fn nanos(time: std::io::Result<SystemTime>) -> i64 {
        time.ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .and_then(|d| i64::try_from(d.as_nanos()).ok())
            .unwrap_or(0)
    }
    (
        nanos(metadata.modified()),
        nanos(metadata.created()),
        i64::try_from(metadata.len()).unwrap_or(i64::MAX),
    )
}

fn refresh(store: &ThreadStore, db: &mut Connection, cancel: &CancelToken) -> IndexResult<()> {
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut known = tx
        .prepare("SELECT id, modified, created, bytes FROM chat")?
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                (row.get(1)?, row.get(2)?, row.get(3)?),
            ))
        })?
        .collect::<rusqlite::Result<HashMap<String, Stamp>>>()?;
    for entry in fs::read_dir(store.dir())? {
        if cancel.is_cancelled() {
            // Keep completed backfill work when another keypress supersedes us.
            tx.commit()?;
            return Ok(());
        }
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|v| v.to_str()) != Some("json")
            || !entry.file_type()?.is_file()
        {
            continue;
        }
        let Some(id) = path
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| ThreadId::parse(s).ok())
        else {
            continue;
        };
        let previous = known.remove(id.as_str());
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if previous == Some(stamp(&metadata)) {
            continue;
        }
        remove_chat(&tx, id.as_str())?;
        let Ok(file) = File::open(&path) else {
            continue;
        };
        // Stamp the open file, not the path: a save can atomically replace it.
        let file_stamp = stamp(&file.metadata()?);
        let Ok(doc) = serde_json::from_reader::<_, Document>(BufReader::new(file)) else {
            continue;
        };
        if doc.version <= THREAD_FORMAT_VERSION && doc.id == id.as_str() {
            insert_chat(&tx, &doc, file_stamp)?;
        }
    }
    for missing in known.keys() {
        remove_chat(&tx, missing)?;
    }
    tx.commit()?;
    Ok(())
}

fn insert_chat(tx: &Transaction<'_>, doc: &Document, stamp: Stamp) -> rusqlite::Result<()> {
    let title = doc
        .title
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("Untitled chat");
    let mut insert = tx.prepare("INSERT INTO line(body, thread, message) VALUES (?1, ?2, ?3)")?;
    let mut first = None;
    for line in &doc.messages {
        let body = match line.role.as_str() {
            "user" | "assistant" => line.text.as_str(),
            "thinking" => line.thinking.as_deref().unwrap_or(&line.text),
            _ => continue,
        };
        if !body.is_empty() {
            insert.execute(params![body, doc.id, line.id])?;
            first.get_or_insert(tx.last_insert_rowid());
        }
    }
    // Store the title last. MIN(rowid) then selects the earliest matching message.
    insert.execute(params![title, doc.id, Option::<String>::None])?;
    let last = tx.last_insert_rowid();
    tx.execute(
        "INSERT INTO chat(id,title,updated,modified,created,bytes,first_row,last_row)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        params![
            doc.id,
            title,
            i64::try_from(doc.updated_at).unwrap_or(i64::MAX),
            stamp.0,
            stamp.1,
            stamp.2,
            first.unwrap_or(last),
            last
        ],
    )?;
    Ok(())
}

fn remove_chat(tx: &Transaction<'_>, id: &str) -> rusqlite::Result<()> {
    let range: Option<(i64, i64)> = tx
        .query_row(
            "SELECT first_row,last_row FROM chat WHERE id=?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if let Some((first, last)) = range {
        tx.execute(
            "DELETE FROM line WHERE rowid BETWEEN ?1 AND ?2",
            [first, last],
        )?;
        tx.execute("DELETE FROM chat WHERE id=?1", [id])?;
    }
    Ok(())
}

pub(crate) fn forget(store: &ThreadStore, id: &ThreadId) -> crate::Result<()> {
    let path = store.dir().join(DATABASE);
    if !path
        .try_exists()
        .map_err(|e| HarnessError::Other(format!("find chat index: {e}")))?
    {
        return Ok(());
    }
    let remove = || -> rusqlite::Result<()> {
        let mut db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        initialize(&db)?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        remove_chat(&tx, id.as_str())?;
        tx.commit()
    };
    remove().map_err(|e| HarnessError::Other(format!("delete indexed chat: {e}")))
}

fn find(db: &Connection, query: &str, include_titles: bool) -> IndexResult<Vec<ChatMatch>> {
    // FTS5 trigrams accelerate literal substrings of three or more characters.
    // Shorter queries use the same substring semantics on indexed visible text.
    let use_fts = query.chars().take(3).count() == 3 && !query.contains('\0');
    let candidate = if use_fts { "line MATCH ?1 AND" } else { "" };
    let sql = format!(
        "WITH matches AS (
           SELECT thread, min(rowid) AS first_match FROM line
           WHERE {candidate} instr(lower(body),lower(?2)) > 0 AND (?3 OR message IS NOT NULL)
           GROUP BY thread
         )
         SELECT chat.id,chat.title,chat.updated,line.message,line.body
         FROM matches JOIN chat ON matches.thread=chat.id JOIN line ON line.rowid=matches.first_match
         ORDER BY chat.updated DESC, chat.modified DESC, chat.id LIMIT ?4"
    );
    let literal = format!("\"{}\"", query.replace('"', "\"\""));
    let mut stmt = db.prepare(&sql)?;
    let rows = stmt.query_map(
        params![literal, query, include_titles, SEARCH_LIMIT as i64],
        |row| {
            let message_id: Option<String> = row.get(3)?;
            let excerpt = if message_id.is_some() {
                match_excerpt(&row.get::<_, String>(4)?, query)
            } else {
                None
            };
            Ok(ChatMatch {
                thread_id: row.get(0)?,
                title: row.get(1)?,
                updated_at: row.get::<_, i64>(2)?.cast_unsigned(),
                message_id,
                excerpt,
            })
        },
    )?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Thread;
    use serde_json::json;

    fn hits(store: &ThreadStore, query: &str) -> Vec<ChatMatch> {
        store
            .search_chats(query, true, &CancelToken::new())
            .unwrap()
    }

    #[test]
    fn backfills_large_legacy_chats_and_only_indexes_visible_fields() {
        let root = tempfile::tempdir().unwrap();
        let store = ThreadStore::open(root.path()).unwrap();
        let thread = json!({
            "version": 3, "id": "large", "title": "Field notes", "updatedAt": 42,
            "messages": [{"role":"assistant","id":"reply","text":"An Æon of \"quoted\" 5%_ matches.",
                          "tools":[{"output":"tool-only-secret"}], "thinking":"hidden-reasoning"},
                         {"role":"thinking","id":"legacy-row","thinking":"A visible historic trace"}],
            "agentMessages": [{"content": format!("wire-only-{}", "x".repeat(8 * 1024 * 1024))}]
        });
        fs::write(
            store.dir().join("large.json"),
            serde_json::to_vec(&thread).unwrap(),
        )
        .unwrap();
        let found = hits(&store, "\"QUOTED\"");
        assert_eq!(found[0].message_id.as_deref(), Some("reply"));
        assert_eq!(found[0].updated_at, 42);
        assert_eq!(
            hits(&store, "historic trace")[0].message_id.as_deref(),
            Some("legacy-row")
        );
        for query in ["Æ", "%_", "5%_"] {
            assert_eq!(hits(&store, query).len(), 1, "{query}");
        }
        for query in ["wire-only-", "hidden-reasoning", "tool-only-secret", "æon"] {
            assert!(hits(&store, query).is_empty(), "{query}");
        }
        assert!(hits(&store, "field")[0].message_id.is_none());
        assert!(store
            .search_chats("field", false, &CancelToken::new())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn refreshes_changed_forked_and_removed_chats_without_writing_on_every_save() {
        let root = tempfile::tempdir().unwrap();
        let store = ThreadStore::open(root.path()).unwrap();
        let mut thread = Thread::new();
        thread.set_title("Old title").unwrap();
        thread.apply_user("first", "A literal needle");
        store.save(&thread).unwrap();
        let path = store.dir().join(DATABASE);
        assert!(!path.exists());
        assert_eq!(hits(&store, "needle").len(), 1);
        let observer = Connection::open(&path).unwrap();
        let revision = || {
            observer
                .query_row("PRAGMA data_version", [], |row| row.get::<_, i64>(0))
                .unwrap()
        };
        let first_revision = revision();
        assert_eq!(
            hits(&store, "literal")[0].message_id.as_deref(),
            Some("first")
        );
        assert_eq!(revision(), first_revision);

        thread.set_title("New title").unwrap();
        thread.apply_user("second", "Fresh words");
        store.save(&thread).unwrap();
        let fork = store.fork(&thread, Some("Fork")).unwrap();
        assert_eq!(revision(), first_revision);
        assert!(hits(&store, "Old title").is_empty());
        assert_eq!(hits(&store, "fresh").len(), 2);
        assert_eq!(hits(&store, "new title")[0].thread_id, thread.id);
        assert_ne!(revision(), first_revision);
        store.delete(&fork.id).unwrap();
        let indexed: i64 = observer
            .query_row(
                "SELECT count(*) FROM line WHERE thread=?1",
                [&fork.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            indexed, 0,
            "delete removes the indexed projection immediately"
        );
        fs::remove_file(store.dir().join(format!("{}.json", thread.id))).unwrap();
        assert!(hits(&store, "fresh").is_empty());
    }

    #[test]
    fn ignores_malformed_future_and_mismatched_documents() {
        let root = tempfile::tempdir().unwrap();
        let store = ThreadStore::open(root.path()).unwrap();
        let path = store.dir().join("chat.json");
        for raw in [
            r#"{"version":999,"id":"chat","title":"needle"}"#,
            r#"{"id":"someone-else","title":"needle"}"#,
            "invalid JSON",
        ] {
            fs::write(&path, raw).unwrap();
            assert!(hits(&store, "needle").is_empty());
        }
        let cancelled = CancelToken::new();
        cancelled.cancel();
        assert!(store
            .search_chats("needle", true, &cancelled)
            .unwrap()
            .is_empty());
    }
}
