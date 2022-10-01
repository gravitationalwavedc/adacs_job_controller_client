// -*- C++ -*- Implement the members of exception_ptr.
// Copyright (C) 2008-2020 Free Software Foundation, Inc.
//
// This file is part of GCC.
//
// GCC is free software; you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation; either version 3, or (at your option)
// any later version.
//
// GCC is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// Under Section 7 of GPL version 3, you are granted additional
// permissions described in the GCC Runtime Library Exception, version
// 3.1, as published by the Free Software Foundation.

// You should have received a copy of the GNU General Public License and
// a copy of the GCC Runtime Library Exception along with this program;
// see the files COPYING3 and COPYING.RUNTIME respectively.  If not, see
// <http://www.gnu.org/licenses/>.

#include "eh_atomics.h"

#define _GLIBCXX_EH_PTR_COMPAT

#include <exception>
#include <bits/exception_ptr.h>
#include "unwind-cxx.h"
#include "my_exception_tracer_lib.h"

using namespace __cxxabiv1;

static void
__gxx_dependent_exception_cleanup(_Unwind_Reason_Code code,
                                  _Unwind_Exception *exc)
{
    // This cleanup is set only for dependents.
    __cxa_dependent_exception *dep = __get_dependent_exception_from_ue (exc);
    __cxa_refcounted_exception *header =
            __get_refcounted_exception_header_from_obj (dep->primaryException);

    // We only want to be called through _Unwind_DeleteException.
    // _Unwind_DeleteException in the HP-UX IA64 libunwind library
    // returns _URC_NO_REASON and not _URC_FOREIGN_EXCEPTION_CAUGHT
    // like the GCC _Unwind_DeleteException function does.
    if (code != _URC_FOREIGN_EXCEPTION_CAUGHT && code != _URC_NO_REASON)
        __terminate (header->exc.terminateHandler);

    __cxa_free_dependent_exception (dep);

    if (__gnu_cxx::__eh_atomic_dec (&header->referenceCount))
    {
        if (header->exc.exceptionDestructor)
            header->exc.exceptionDestructor (header + 1);

        __cxa_free_exception (header + 1);
    }
}

namespace __exception_ptr {
    class my_exception_ptr {
    public:
        void* _M_exception_object;

        void *_M_get() const _GLIBCXX_NOEXCEPT __attribute__ ((__pure__)) {
            return _M_exception_object;
        }
    };
}

void my_rethrow_exception(std::exception_ptr _ep)
{
    getRethrowExceptionCallbacks().invoke(_ep);

    // Bullshit hacks.
    auto *ep = reinterpret_cast<__exception_ptr::my_exception_ptr*>(&_ep);

    void *obj = ep->_M_get();
    __cxa_refcounted_exception *eh
            = __get_refcounted_exception_header_from_obj (obj);

    __cxa_dependent_exception *dep = __cxa_allocate_dependent_exception ();
    dep->primaryException = obj;
    __gnu_cxx::__eh_atomic_inc (&eh->referenceCount);

    dep->unexpectedHandler = std::get_unexpected ();
    dep->terminateHandler = std::get_terminate ();
    __GXX_INIT_DEPENDENT_EXCEPTION_CLASS(dep->unwindHeader.exception_class);
    dep->unwindHeader.exception_cleanup = __gxx_dependent_exception_cleanup;

    __cxa_eh_globals *globals = __cxa_get_globals ();
    globals->uncaughtExceptions += 1;

#ifdef __USING_SJLJ_EXCEPTIONS__
    _Unwind_SjLj_RaiseException (&dep->unwindHeader);
#else
    _Unwind_RaiseException (&dep->unwindHeader);
#endif

    // Some sort of unwinding error.  Note that terminate is a handler.
    __cxa_begin_catch (&dep->unwindHeader);
    std::terminate();
}

#undef _GLIBCXX_EH_PTR_COMPAT