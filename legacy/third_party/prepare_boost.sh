#!/bin/bash

rm -Rf boost_install
rm -Rf boost

cd ../..

# Needs to run from top level project directory for old git
git submodule update --init --recursive

cd src/third_party/boost

export BOOST_VER=boost-1.83.0
git fetch --all --tags
git checkout --force "$BOOST_VER"
git submodule foreach '(git fetch --all --tags && git checkout --force "$BOOST_VER") || true'

./bootstrap.sh --prefix=`pwd`/../boost_install/
./b2 install -j `nproc` --with-test --with-system --with-thread --with-coroutine --with-context --with-regex --with-filesystem --with-url
