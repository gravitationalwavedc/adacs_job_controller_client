use parking_lot::RwLock;
use std::collections::HashMap;
use std::thread::ThreadId;
use tracing::trace;

pub static THREAD_BUNDLE_HASH_MAP: std::sync::LazyLock<RwLock<HashMap<ThreadId, String>>> =
    std::sync::LazyLock::new(|| RwLock::new(HashMap::new()));

pub fn set_current_thread_bundle(bundle_hash: String) {
    let thread_id = std::thread::current().id();
    trace!(
        "thread_bundle_map: set bundle='{}' for thread {:?}",
        bundle_hash,
        thread_id
    );
    THREAD_BUNDLE_HASH_MAP
        .write()
        .insert(thread_id, bundle_hash);
}

pub fn clear_current_thread_bundle() {
    let thread_id = std::thread::current().id();
    trace!(
        "thread_bundle_map: clearing bundle for thread {:?}",
        thread_id
    );
    THREAD_BUNDLE_HASH_MAP.write().remove(&thread_id);
}

pub fn get_current_thread_bundle() -> Option<String> {
    let thread_id = std::thread::current().id();
    let result = THREAD_BUNDLE_HASH_MAP.read().get(&thread_id).cloned();
    if result.is_some() {
        trace!(
            "thread_bundle_map: get bundle='{:?}' for thread {:?}",
            result,
            thread_id
        );
    } else {
        trace!("thread_bundle_map: no bundle for thread {:?}", thread_id);
    }
    result
}

/// RAII guard that sets the current thread bundle on creation
/// and clears it on drop, even if the scope panics.
pub struct ThreadBundleGuard {
    bundle_hash: String,
}

impl ThreadBundleGuard {
    pub fn new(bundle_hash: String) -> Self {
        set_current_thread_bundle(bundle_hash.clone());
        Self { bundle_hash }
    }
}

impl Drop for ThreadBundleGuard {
    fn drop(&mut self) {
        clear_current_thread_bundle();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_current_thread_bundle_replaces_existing_hash_for_same_thread() {
        clear_current_thread_bundle();

        set_current_thread_bundle("first-hash".to_string());
        assert_eq!(get_current_thread_bundle(), Some("first-hash".to_string()));

        set_current_thread_bundle("second-hash".to_string());
        assert_eq!(get_current_thread_bundle(), Some("second-hash".to_string()));

        clear_current_thread_bundle();
    }
}
