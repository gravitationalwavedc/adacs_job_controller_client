FROM centos:7.9.2009 AS build_base

RUN yum -y upgrade && yum -y install epel-release
RUN yum install -y gcc gcc-c++ make python3-devel cmake3 glibc-static libstdc++-static git

# Copy in the source directory
ADD src /src

# Prepare boost
RUN cd /src/third_party && bash prepare_boost.sh

# Set up the build directory and configure the project with cmake
RUN mkdir /src/build
WORKDIR /src/build
RUN cmake3 -DCMAKE_BUILD_TYPE=Debug ..

# Build dependencies
#RUN cmake --build . --target folly -- -j `nproc`
#RUN cmake --build . --target folly_exception_tracer -- -j `nproc`
#RUN cmake --build . --target folly_exception_counter -- -j `nproc`


FROM build_base AS build_production

# Build the production server
RUN cmake3 --build . --target adacs_job_client -- -j `nproc`

ENTRYPOINT cp ./adacs_job_client /out/ && chown 1000:1000 /out/adacs_job_client
