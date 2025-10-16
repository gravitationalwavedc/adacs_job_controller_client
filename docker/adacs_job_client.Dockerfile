FROM centos:7.9.2009 AS build_base

# Point stock CentOS repos to vault
RUN sed -ri 's|^mirrorlist=|#mirrorlist=|g; s|^#?baseurl=http://mirror.centos.org|baseurl=http://vault.centos.org|g' /etc/yum.repos.d/CentOS-*.repo \
 && yum -y upgrade

# Install SCL and EPEL, then repoint the newly-created SCLo repos to vault too
RUN yum -y install centos-release-scl epel-release 

# Replace SCLo repos with vault, and use the **SCLo SIG** GPG key
RUN rm -f /etc/yum.repos.d/CentOS-SCLo-*.repo && cat >/etc/yum.repos.d/CentOS-SCLo-vault.repo <<'EOF'
[centos-sclo-rh]
name=CentOS-7 - SCLo rh (vault)
baseurl=https://vault.centos.org/7.9.2009/sclo/$basearch/rh/
enabled=1
gpgcheck=1
# This key signs devtoolset-* packages
gpgkey=file:///etc/pki/rpm-gpg/RPM-GPG-KEY-CentOS-SIG-SCLo

[centos-sclo-sclo]
name=CentOS-7 - SCLo sclo (vault)
baseurl=https://vault.centos.org/7.9.2009/sclo/$basearch/sclo/
enabled=1
gpgcheck=1
gpgkey=file:///etc/pki/rpm-gpg/RPM-GPG-KEY-CentOS-SIG-SCLo
EOF

# EPEL 7 -> Fedora archives (mirrorlist/metalink no longer works)
RUN sed -ri 's|^metalink=|#metalink=|g; s|^mirrorlist=|#mirrorlist=|g' /etc/yum.repos.d/epel*.repo \
 && sed -ri 's|^#?baseurl=.*epel/.*$|baseurl=https://archives.fedoraproject.org/pub/archive/epel/7/$basearch/|g' /etc/yum.repos.d/epel*.repo

RUN yum -y install devtoolset-11 \
 && yum -y install python3-devel cmake3 git ninja-build autoconf gettext-devel automake flex bison \
 && yum clean all && rm -rf /var/cache/yum

# Copy in the source directory
ADD . /project/

# Prepare boost and third party libs
RUN cd /project/src/third_party && source /opt/rh/devtoolset-11/enable && bash prepare_elfutils.sh && bash prepare_boost.sh

# Install fast_float (header-only) so Folly's FindFastFloat can locate it
RUN git clone --depth=1 https://github.com/fastfloat/fast_float.git /tmp/fast_float \
 && cp -r /tmp/fast_float/include/fast_float /usr/include/ \
 && rm -rf /tmp/fast_float

# Set up the build directory and configure the project with cmake
RUN mkdir /project/src/build
WORKDIR /project/src/build
RUN source /opt/rh/devtoolset-11/enable && which cmake3 && /usr/bin/cmake3 -DCMAKE_BUILD_TYPE=Release -DCMAKE_MAKE_PROGRAM=/usr/bin/ninja -G Ninja ..

FROM build_base AS build_production

# Build the production server
RUN source /opt/rh/devtoolset-11/enable && cmake3 --build . --target adacs_job_client -- -j `nproc`

ENTRYPOINT cp ./adacs_job_client /out/ && chown 1000:1000 /out/adacs_job_client
