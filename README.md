# ADACS Job Controller Client

## Project setup

1. Clone the repository `git clone https://gitlab.com/CAS-eResearch/GWDC/code/adacs_job_controller_client.git`
2. Init all submodules `git submodule update --init --recursive`
3. Initialise the boost installation `cd src/third_party && bash prepare_boost.sh`
4. Initialise the elfutils installation `cd src/third_party && bash prepare_elfutils.h`



## Building

#### For Development

The project can now be built. There are two primary targets: `Boost_Tests_run`, and `adacs_job_client`. You'll likely only want to bother building `Boost_Tests_run` and using this to test the functionality of the job client.

1. Make a build directory `cd src && mkdir build`
2. Change in to the build directory and run the cmake configure `cd build && cmake -DCMAKE_BUILD_TYPE=Debug ..`
3. Build the target `cmake --build . --target Boost_Tests_run`



NB. To run the full test suite, it may be possible that a db.sqlite3 database is required with prepopulated tables. To do this, `cd src/Lib/schema && . venv/bin/activate && python manage.py migrate` then copy the db.sqlite3 database to wherever the `Boost_Tests_run` binary is.



#### For Deployment

Since a primary objective of this project is to statically link a binary that doesn't rely on any system libraries (Except core libc/libdl etc) and that can run on ancient glibc versions, we use a docker build using centos 7 to build the production binary. There is a convenience script for this.

`bash scripts/build.sh`

This will generate a directory `out` which contains the executable `adacs_job_client` which can be shipped and deployed.



