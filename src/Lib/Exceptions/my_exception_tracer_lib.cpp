// NOLINTBEGIN

#include "segvcatch.h"
#include "my_exception_tracer_lib.h"
#include <bits/exception_ptr.h>
#include <execinfo.h>
#include <folly/Indestructible.h>
#include <folly/experimental/exception_tracer/StackTrace.h>
#include <memory_patch.h>


// Disable all optimisations in this file. Enabling optimisations here can lead to all kinds of wild startup segfaults
// due to our exception handling function override hacks.
#pragma GCC optimize 0


namespace __cxxabiv1 {
    extern "C" {
        void __cxa_throw(void *thrownException, std::type_info *type, void (*destructor)(void *));

        void __cxa_rethrow();

        void *__cxa_begin_catch(void *excObj) throw();

        void __cxa_end_catch();
    }
}

namespace std {
    void rethrow_exception(std::exception_ptr ep);
}

extern "C" {
    void my__cxa_throw(void *thrownException, std::type_info *type, void (*destructor)(void *));
    void my__cxa_rethrow();
    void *my__cxa_begin_catch(void *excObj) throw();
    void my__cxa_end_catch();
}

void my_rethrow_exception(std::exception_ptr ep);

volatile memory_patch::TrampolineHook<void(void*, std::type_info*, void (*)(void *))> my__cxa_throw_patch(__cxxabiv1::__cxa_throw, my__cxa_throw);
volatile memory_patch::TrampolineHook<void()> my__cxa_rethrow_patch(__cxxabiv1::__cxa_rethrow, my__cxa_rethrow);
volatile memory_patch::TrampolineHook<void*(void*)> my__cxa_begin_catch_patch(__cxxabiv1::__cxa_begin_catch, my__cxa_begin_catch);
volatile memory_patch::TrampolineHook<void(void)> my__cxa_end_catch_patch(__cxxabiv1::__cxa_end_catch, my__cxa_end_catch);
volatile memory_patch::TrampolineHook<void(std::exception_ptr)> my_rethrow_exception_patch(std::rethrow_exception, my_rethrow_exception);

void handleSegv()
{
    // NOLINTBEGIN
    void *array[10];
    int size;

    // get void*'s for all entries on the stack
    size = backtrace(array, 10);

    // print out all the frames to stderr
    fprintf(stderr, "Error: SEGFAULT:\n");
    backtrace_symbols_fd(array, size, STDERR_FILENO);

    google::FlushLogFilesUnsafe(google::INFO);
    fflush(stdout);
    fflush(stderr);

    throw std::runtime_error("Seg Fault Error");
    // NOLINTEND
}

void handleFpe()
{
    // NOLINTBEGIN
    void *array[10];
    int size;

    // get void*'s for all entries on the stack
    size = backtrace(array, 10);

    // print out all the frames to stderr
    fprintf(stderr, "Error: SEGFPE:\n");
    backtrace_symbols_fd(array, size, STDERR_FILENO);

    google::FlushLogFilesUnsafe(google::INFO);
    fflush(stdout);
    fflush(stderr);

    throw std::runtime_error("Floating Point Error");
    // NOLINTEND
}

auto beforeMain() -> bool {
    // Set up the crash handler
    segvcatch::init_segv(&handleSegv);
    segvcatch::init_fpe(&handleFpe);
}

// Enforce the early segv handler
volatile bool _beforeMain = beforeMain();

namespace folly {
    namespace exception_tracer {
#define FOLLY_EXNTRACE_DECLARE_CALLBACK(NAME)                    \
  extern "C" CallbackHolder<NAME##Type>& get##NAME##Callbacks() {           \
    static Indestructible<CallbackHolder<NAME##Type>> Callbacks; \
    return *Callbacks;                                           \
  }                                                              \
  void register##NAME##Callback(NAME##Type callback) {           \
    get##NAME##Callbacks().registerCallback(callback);           \
  }

        FOLLY_EXNTRACE_DECLARE_CALLBACK(CxaThrow)
        FOLLY_EXNTRACE_DECLARE_CALLBACK(CxaBeginCatch)
        FOLLY_EXNTRACE_DECLARE_CALLBACK(CxaRethrow)
        FOLLY_EXNTRACE_DECLARE_CALLBACK(CxaEndCatch)
        FOLLY_EXNTRACE_DECLARE_CALLBACK(RethrowException)

#undef FOLLY_EXNTRACE_DECLARE_CALLBACK
    }
}

// To prevent the compiler optimizing away the exception tracing from folly, we need to reference it.
extern "C" auto getCaughtExceptionStackTraceStack() -> const folly::exception_tracer::StackTrace*;
extern "C" auto getUncaughtExceptionStackTraceStack() -> const folly::exception_tracer::StackTraceStack*;

// forceExceptionStackTraceRef is intentionally unused and marked volatile so the compiler doesn't optimize away the
// required functions from folly. This is black magic.
volatile void forceExceptionStackTraceRef()
{
    getCaughtExceptionStackTraceStack();
    getUncaughtExceptionStackTraceStack();
}

// NOLINTEND

#pragma GCC reset_options