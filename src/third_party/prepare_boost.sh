#!/bin/bash

cd boost

export BOOST_VER=boost-1.80.0
git checkout --force "$BOOST_VER"
git submodule update --init --recursive --depth=1
git submodule foreach 'git checkout --force "$BOOST_VER" || true'

./bootstrap.sh --prefix=`pwd`/../boost_install/
./b2 install -j `nproc` --with-test --with-system --with-thread --with-coroutine --with-context --with-regex --with-filesystem
