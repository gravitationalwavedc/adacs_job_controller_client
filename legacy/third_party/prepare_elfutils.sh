#!/bin/bash

rm -Rf elfutils_install
rm -Rf elfutils

cd ../..

# Needs to run from top level project directory for old git
git submodule update --init --recursive

cd src/third_party/elfutils

autoreconf -fiv

./configure --prefix=`pwd`/../elfutils_install/ --enable-maintainer-mode --disable-nls --without-valgrind --without-bzlib --without-lzma --without-zstd --without-libiconv-prefix --without-libintl-prefix --enable-libdebuginfod=dummy --disable-debuginfod

make -j8 install
