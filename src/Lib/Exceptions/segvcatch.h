//
// Created by lewis on 5/4/20.
//

#ifndef GWCLOUD_JOB_SERVER_SEGVCATCH_H
#define GWCLOUD_JOB_SERVER_SEGVCATCH_H

/***************************************************************************
 *   Copyright (C) 2009 by VisualData                                      *
 *                                                                         *
 *   Redistributed under LGPL license terms.                               *
 ***************************************************************************/

/*! \brief segvcatch namespace

 // NOLINTBEGIN

*/
namespace segvcatch
{

/*! Signal handler, used to redefine standart exception throwing. */
    typedef void (*handler)();

/*! Initialize segmentation violation handler.
    \param h (optional) - optional user's signal handler. By default used an internal signal handler to throw
 std::runtime_error.
   */
    void init_segv(handler h = 0);

/*! Initialize floating point error handler.
    \param h - optional user's signal handler. By default used an internal signal handler to throw
 std::runtime_error.*/
    void init_fpe(handler h = 0);

}

// NOLINTEND
#endif //GWCLOUD_JOB_SERVER_SEGVCATCH_H
