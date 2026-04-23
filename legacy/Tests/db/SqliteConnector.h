//
// Created by lewis on 5/3/20.
//

#ifndef GWCLOUD_JOB_SERVER_SQLITELCONNECTOR_H
#define GWCLOUD_JOB_SERVER_SQLITELCONNECTOR_H

#ifdef BUILD_TESTS

#include "../../Lib/GeneralUtils.h"
#include "../../Settings.h"
#include <sqlpp11/sqlite3/connection.h>
#include <sqlpp11/sqlite3/connection_config.h>
#include <sqlpp11/sqlpp11.h>

namespace sqlite = sqlpp::sqlite3;

class SqliteConnector {
public:
    SqliteConnector() {
        sqlite::connection_config config;
        config.path_to_database = (getExecutablePath() / DATABASE_FILE).string();
        config.flags = SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE; // NOLINT(hicpp-signed-bitwise)
        config.debug = false;
        database = std::make_shared<sqlite::connection>(config);
    }

    virtual ~SqliteConnector() = default;
    SqliteConnector(SqliteConnector const&) = delete;
    auto operator =(SqliteConnector const&) -> SqliteConnector& = delete;
    SqliteConnector(SqliteConnector&&) = delete;
    auto operator=(SqliteConnector&&) -> SqliteConnector& = delete;

    auto operator->() const -> std::shared_ptr<sqlite::connection>
    { return database; }

    [[nodiscard]] auto getDb() const -> std::shared_ptr<sqlite::connection>
    { return database; }

private:
    std::shared_ptr<sqlite::connection> database;
};

#endif

#endif //GWCLOUD_JOB_SERVER_SQLITELCONNECTOR_H

