use lazy_static::lazy_static;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::thread::ThreadId;

lazy_static! {
    pub static ref THREAD_BUNDLE_HASH_MAP: RwLock<HashMap<ThreadId, String>> =
        RwLock::new(HashMap::new());
}

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
