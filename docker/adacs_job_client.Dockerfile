FROM centos:7.9.2009 AS build_base

RUN yum -y upgrade && yum -y install centos-release-scl epel-release && yum -y install devtoolset-11
RUN yum install -y python3-devel cmake3 git ninja-build autoconf gettext-devel automake flex bison

# Copy in the third party directory
ADD src/third_party /src/third_party

# Prepare boost and third party libs
RUN cd /src/third_party && source /opt/rh/devtoolset-11/enable && bash prepare_elfutils.sh && bash prepare_boost.sh && mv /src/third_party /

# Copy in the source directory
ADD src /src

# Move our third_party in place of the one in the source tree
RUN rm -rf /src/third_party && mv /third_party /src/

# Set up the build directory and configure the project with cmake
RUN mkdir /src/build
WORKDIR /src/build
RUN source /opt/rh/devtoolset-11/enable && which cmake3 && /usr/bin/cmake3 -DCMAKE_BUILD_TYPE=Debug -DCMAKE_MAKE_PROGRAM=/usr/bin/ninja -G Ninja ..

# Build dependencies
#RUN cmake --build . --target folly -- -j `nproc`
#RUN cmake --build . --target folly_exception_tracer -- -j `nproc`
#RUN cmake --build . --target folly_exception_counter -- -j `nproc`


FROM build_base AS build_production

# Build the production server
RUN source /opt/rh/devtoolset-11/enable && cmake3 --build . --target adacs_job_client -- -j `nproc`

ENTRYPOINT cp ./adacs_job_client /out/ && chown 1000:1000 /out/adacs_job_client
