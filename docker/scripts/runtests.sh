#! /bin/bash
set -e
# /src is already the working directory (set in dockerfile)

# Wait for mysql to be ready
while ! mysqladmin ping -h"$DATABASE_HOST" --silent; do
    sleep 1
done

# Migrate the database
utils/schema/venv/bin/python utils/schema/manage.py migrate;

# Run the jobserver tests
build/Boost_Tests_run --catch_system_error=yes --log_format=JUNIT --show_progress=no --log_sink=/test_report/junit.xml

build/Boost_Tests_run --logger=HRF,all --color_output=true --report_format=HRF --show_progress=no

# Print some whitespace
printf "\n\n"

# Generate code coverage reports
gcovr -r . --xml-pretty -e "third_party/" > /test_report/coverage_docker.xml

gcovr -r . -e "third_party/"

python3 utils/clang-tidy-to-code-climate.py /src/build/tidy.txt /test_report/code_climate.json /

# Set full access on the test_report directory in case, so the host can delete the files if needed
chmod -R 666 /test_report
chmod 777 /test_report
