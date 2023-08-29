FROM centos:7.9.2009 AS build_base


RUN ulimit -n 1024000 && yum -y upgrade && yum -y install centos-release-scl epel-release && yum -y install devtoolset-11
RUN ulimit -n 1024000 && yum install -y python3-devel cmake3 git ninja-build autoconf gettext-devel automake flex bison

# Copy in the source directory
ADD . /project/

# Prepare boost and third party libs
RUN cd /project/src/third_party && source /opt/rh/devtoolset-11/enable && bash prepare_elfutils.sh && bash prepare_boost.sh

# Set up the build directory and configure the project with cmake
RUN mkdir /project/src/build
WORKDIR /project/src/build
RUN source /opt/rh/devtoolset-11/enable && which cmake3 && /usr/bin/cmake3 -DCMAKE_BUILD_TYPE=Release -DCMAKE_MAKE_PROGRAM=/usr/bin/ninja -G Ninja ..

FROM build_base AS build_production

# Build the production server
RUN source /opt/rh/devtoolset-11/enable && cmake3 --build . --target adacs_job_client -- -j `nproc`

ENTRYPOINT cp ./adacs_job_client /out/ && chown 1000:1000 /out/adacs_job_client
