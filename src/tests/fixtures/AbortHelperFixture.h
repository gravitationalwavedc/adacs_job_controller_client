//
// Created by lewis on 9/5/22.
//

#ifndef ADACS_JOB_CLIENT_ABORTHELPERFIXTURE_H
#define ADACS_JOB_CLIENT_ABORTHELPERFIXTURE_H

extern bool applicationAborted;

class AbortHelperFixture {
public:
    AbortHelperFixture() {
        applicationAborted = false;
    }

    ~AbortHelperFixture() {
        applicationAborted = false;
    }

    bool checkAborted() {
        return applicationAborted;
    }
};

#endif //ADACS_JOB_CLIENT_ABORTHELPERFIXTURE_H
