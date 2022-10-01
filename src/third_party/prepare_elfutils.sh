#!/bin/bash

cd elfutils

autoreconf -fiv

./configure --prefix=`pwd`/../elfutils_install/ --enable-maintainer-mode --disable-nls --without-valgrind --without-bzlib --without-lzma --without-zstd --without-libiconv-prefix --without-libintl-prefix

make -j8 install