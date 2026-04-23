//
// Thread-safe mapping of thread IDs to bundle hashes
//
// This header provides a global map that tracks which bundle hash is currently
// being executed by each thread. This is used by bundle-related modules to
// determine the context of the current Python execution.
//

#ifndef ADACS_JOB_CLIENT_THREADBUNDLEMAP_H
#define ADACS_JOB_CLIENT_THREADBUNDLEMAP_H

#include <map>
#include <shared_mutex>
#include <string>
#include <thread>

// Global map and mutex for thread-safe access
extern std::map<std::thread::id, std::string> threadBundleHashMap;
extern std::shared_mutex threadBundleHashMapMutex;

#endif //ADACS_JOB_CLIENT_THREADBUNDLEMAP_H
