#! /bin/bash
set -e
# /src is already the working directory (set in dockerfile)

# Wait for mysql to be ready
while ! mysqladmin ping -h"$DATABASE_HOST" --silent; do
    sleep 1
done

# Migrate the database
utils/schema/venv/bin/python utils/schema/manage.py migrate;

# Run the valgrind tests
/usr/bin/valgrind --tool=memcheck --gen-suppressions=all --leak-resolution=med --track-origins=yes --vgdb=no --leak-check=full --suppressions=./.valgrind.suppressed build/Boost_Tests_run --logger=HRF,all --color_output=true --report_format=HRF --show_progress=no
