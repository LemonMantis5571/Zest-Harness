use serde::Deserialize;
use std::io::Read;
use zest_core::{THREAD_FORMAT_VERSION, ThreadId, ThreadStore, ThreadSummary, match_excerpt};

#[derive(Clone)]
pub struct Hit {
    pub thread: String,
    pub title: String,
    pub message: String,
    pub excerpt: String,
}

// Only the visible projection is searchable. Provider wire history can contain unrelated payloads.
#[derive(Deserialize)]
struct Document {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    messages: Vec<Line>,
}
#[derive(Deserialize)]
struct Line {
    id: String,
    #[serde(default)]
    text: String,
}

pub fn search(
    store: &ThreadStore,
    history: &[ThreadSummary],
    query: &str,
    cancel: &zest_core::CancelToken,
) -> Vec<Hit> {
    let mut hits = Vec::new();
    if query.trim().is_empty() {
        return hits;
    }
    for row in history {
        if cancel.is_cancelled() {
            break;
        }
        let Ok(id) = ThreadId::parse(&row.id) else {
            continue;
        };
        let path = store.dir().join(format!("{id}.json"));
        const MAX_BYTES: u64 = 8 * 1024 * 1024;
        let Ok(file) = std::fs::File::open(path) else {
            continue;
        };
        if !file
            .metadata()
            .is_ok_and(|m| m.is_file() && m.len() <= MAX_BYTES)
        {
            continue;
        }
        let mut body = Vec::new();
        if file.take(MAX_BYTES + 1).read_to_end(&mut body).is_err() || body.len() as u64 > MAX_BYTES
        {
            continue;
        }
        let Ok(doc) = serde_json::from_slice::<Document>(&body) else {
            continue;
        };
        if doc.version > THREAD_FORMAT_VERSION {
            continue;
        }
        if let Some((line, excerpt)) = doc
            .messages
            .iter()
            .find_map(|line| match_excerpt(&line.text, query).map(|s| (line, s)))
        {
            hits.push(Hit {
                thread: row.id.clone(),
                title: row.title.clone().unwrap_or_else(|| "Untitled chat".into()),
                message: line.id.clone(),
                excerpt,
            });
            if hits.len() == 24 {
                break;
            }
        }
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::search;
    use zest_core::{CancelToken, Thread, ThreadStore};

    #[test]
    fn superseded_query_stops_before_searching_a_saved_chat() {
        let folder = tempfile::tempdir().unwrap();
        let store = ThreadStore::open(folder.path()).unwrap();
        let mut thread = Thread::new();
        thread.apply_user("q", "needle in a saved conversation");
        store.save(&thread).unwrap();
        let history = [thread.summary()];
        let cancel = CancelToken::new();
        assert_eq!(search(&store, &history, "needle", &cancel)[0].message, "q");
        cancel.cancel();
        assert!(search(&store, &history, "needle", &cancel).is_empty());
    }
}
