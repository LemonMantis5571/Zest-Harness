use zest_core::{CancelToken, ThreadStore};

#[derive(Clone)]
pub struct Hit {
    pub thread: String,
    pub title: String,
    pub message: String,
    pub excerpt: String,
}

pub fn search(
    store: &ThreadStore,
    query: &str,
    cancel: &CancelToken,
) -> zest_core::Result<Vec<Hit>> {
    Ok(store
        .search_chats(query, false, cancel)?
        .into_iter()
        .filter_map(|hit| {
            Some(Hit {
                thread: hit.thread_id,
                title: hit.title,
                message: hit.message_id?,
                excerpt: hit.excerpt?,
            })
        })
        .collect())
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
        let cancel = CancelToken::new();
        assert_eq!(search(&store, "needle", &cancel).unwrap()[0].message, "q");
        cancel.cancel();
        assert!(search(&store, "needle", &cancel).unwrap().is_empty());
    }
}
