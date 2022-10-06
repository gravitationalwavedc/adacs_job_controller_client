//
// Created by lewis on 9/3/22.
// Heavily adapted from
// https://github.com/sterin/python-sub-interpreters-multiple-threads-example/blob/master/main.cpp
//

#ifndef ADACS_JOB_CLIENT_PYTHONINTERFACE_H
#define ADACS_JOB_CLIENT_PYTHONINTERFACE_H


#include <cassert>
#include <string>

#include <Python.h>
#include <memory>
#include <shared_mutex>
#include <mutex>

class PythonInterface {
public:
    static void initPython(const std::string& sPythonLibrary);
    static auto getPythonLibHandle() -> void*;
    static auto MyPy_IsNone(PyObject* obj) -> bool;

private:
    class RestoreThreadStateScope
    {
    public:
        RestoreThreadStateScope()
        {
            _ts = PyThreadState_Get();
        }

        ~RestoreThreadStateScope()
        {
            PyThreadState_Swap(_ts);
        }

    private:

        PyThreadState* _ts;
    };

// swap the current thread state with ts, restore when the object goes out of scope
    class SwapThreadStateScope
    {
    public:
        explicit SwapThreadStateScope(PyThreadState* ts)
        {
            _ts = PyThreadState_Swap(ts);
        }

        ~SwapThreadStateScope()
        {
            PyThreadState_Swap(_ts);
        }

    private:

        PyThreadState* _ts;
    };

// create new thread state for interpreter interp, make it current, and clean up on destruction
    class ThreadState
    {
    public:

        explicit ThreadState(PyInterpreterState* interp)
        {
            _ts = PyThreadState_New(interp);
            PyEval_RestoreThread(_ts);
        }

        ~ThreadState()
        {
            PyThreadState_Clear(_ts);
            PyThreadState_DeleteCurrent();
        }

        operator PyThreadState*()
        {
            return _ts;
        }

        static PyThreadState* current()
        {
            return PyThreadState_Get();
        }

    private:

        PyThreadState* _ts;
    };

public:
    class SubInterpreter
    {
    public:

        // perform the necessary setup and cleanup for a new thread running using a specific interpreter
        struct ThreadScope
        {
            ThreadState _state;
            SwapThreadStateScope _swap{_state };

            ThreadScope(PyInterpreterState* interp) :
                    _state(interp)
            {
            }
        };

        SubInterpreter()
        {
            RestoreThreadStateScope restore;

            _ts = Py_NewInterpreter();
        }

        ~SubInterpreter()
        {
            if( _ts )
            {
                SwapThreadStateScope sts(_ts);

                Py_EndInterpreter(_ts);
            }
        }

        PyInterpreterState* interp()
        {
            return _ts->interp;
        }

        static PyInterpreterState* current()
        {
            return ThreadState::current()->interp;
        }

    private:

        PyThreadState* _ts;
    };

    static auto newInterpreter() -> std::shared_ptr<SubInterpreter>;
};


#endif //ADACS_JOB_CLIENT_PYTHONINTERFACE_H
