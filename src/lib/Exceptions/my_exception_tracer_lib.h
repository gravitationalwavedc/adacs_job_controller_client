#ifndef ADACS_JOB_CLIENT_MY_EXCEPTION_TRACER_LIB_H
#define ADACS_JOB_CLIENT_MY_EXCEPTION_TRACER_LIB_H

#include <folly/experimental/exception_tracer/ExceptionTracerLib.h>

#include <vector>

#include <folly/Indestructible.h>
#include <folly/Portability.h>
#include <folly/SharedMutex.h>
#include <folly/Synchronized.h>

using namespace folly::exception_tracer;

namespace {

    template <typename Function>
    class CallbackHolder {
    public:
        void registerCallback(Function f) {
            callbacks_.wlock()->push_back(std::move(f));
        }

        // always inline to enforce kInternalFramesNumber
        template <typename... Args>
        FOLLY_ALWAYS_INLINE void invoke(Args... args) {
            auto callbacksLock = callbacks_.rlock();
            for (auto& cb : *callbacksLock) {
                cb(args...);
            }
        }

    private:
        folly::Synchronized<std::vector<Function>> callbacks_;
    };

} // namespace

namespace folly {
    namespace exception_tracer {

#define FOLLY_EXNTRACE_DECLARE_CALLBACK_DEF(NAME)                    \
  extern "C" CallbackHolder<NAME##Type>& get##NAME##Callbacks();            \
  void register##NAME##Callback(NAME##Type callback);

        FOLLY_EXNTRACE_DECLARE_CALLBACK_DEF(CxaThrow)
        FOLLY_EXNTRACE_DECLARE_CALLBACK_DEF(CxaBeginCatch)
        FOLLY_EXNTRACE_DECLARE_CALLBACK_DEF(CxaRethrow)
        FOLLY_EXNTRACE_DECLARE_CALLBACK_DEF(CxaEndCatch)
        FOLLY_EXNTRACE_DECLARE_CALLBACK_DEF(RethrowException)

#undef FOLLY_EXNTRACE_DECLARE_CALLBACK_DEF

    } // namespace exception_tracer
} // namespace folly

#endif //ADACS_JOB_CLIENT_MY_EXCEPTION_TRACER_LIB_H
