#include <stdio.h>

typedef enum { PyGILState_LOCKED, PyGILState_UNLOCKED } PyGILState_STATE;

typedef PyGILState_STATE (*ensure_fn)(void);
typedef void (*release_fn)(PyGILState_STATE);

void trigger_hooks(void* ensure_ptr, void* release_ptr) {
    ensure_fn ensure = (ensure_fn)ensure_ptr;
    release_fn release = (release_fn)release_ptr;

    printf("C: Calling PyGILState_Ensure through pointer %p...\n", ensure_ptr);
    PyGILState_STATE state = ensure();
    printf("C: Calling PyGILState_Release through pointer %p...\n", release_ptr);
    release(state);
}
