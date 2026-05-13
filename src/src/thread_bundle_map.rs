use parking_lot::RwLock;
use std::collections::HashMap;
use std::thread::ThreadId;

pub static THREAD_BUNDLE_HASH_MAP: std::sync::LazyLock<RwLock<HashMap<ThreadId, String>>> =
    std::sync::LazyLock::new(|| RwLock::new(HashMap::new()));

pub fn set_current_thread_bundle(bundle_hash: String) {
    THREAD_BUNDLE_HASH_MAP
        .write()
        .insert(std::thread::current().id(), bundle_hash);
}

pub fn clear_current_thread_bundle() {
    THREAD_BUNDLE_HASH_MAP
        .write()
        .remove(&std::thread::current().id());
}

pub fn get_current_thread_bundle() -> Option<String> {
    THREAD_BUNDLE_HASH_MAP
        .read()
        .get(&std::thread::current().id())
        .cloned()
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
