//
// Created by lewis on 7/25/22.
//

#ifndef GWCLOUD_JOB_SERVER_DATABASEFIXTURE_H
#define GWCLOUD_JOB_SERVER_DATABASEFIXTURE_H

#include "../../DB/SqliteConnector.h"
#include "../../lib/jobclient_schema.h"

struct DatabaseFixture {
    // NOLINTBEGIN(misc-non-private-member-variables-in-classes)
    SqliteConnector database;

    schema::JobclientJob jobTable{};
    schema::JobclientJobstatus statusTable{};
    // NOLINTEND(misc-non-private-member-variables-in-classes)

    DatabaseFixture() {
        cleanDatabase();
    }

    // NOLINTNEXTLINE(bugprone-exception-escape)
    ~DatabaseFixture() {
        cleanDatabase();
    }

    DatabaseFixture(DatabaseFixture const&) = delete;
    auto operator =(DatabaseFixture const&) -> DatabaseFixture& = delete;
    DatabaseFixture(DatabaseFixture&&) = delete;
    auto operator=(DatabaseFixture&&) -> DatabaseFixture& = delete;

private:
    void cleanDatabase() const {
        // Sanitize all records from the database
        database->operator()(remove_from(statusTable).unconditionally());
        database->operator()(remove_from(jobTable).unconditionally());
    }
};

#endif //GWCLOUD_JOB_SERVER_DATABASEFIXTURE_H
