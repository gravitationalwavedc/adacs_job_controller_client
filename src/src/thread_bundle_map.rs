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
