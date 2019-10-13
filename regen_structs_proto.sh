#!/bin/sh

# This script regenerates the `src/protobuf_structs/btt.rs` file from `btt.proto`.

docker run --rm -v `pwd`:/usr/code:z -w /usr/code rust /bin/bash -c " \
    apt-get update; \
    apt-get install -y protobuf-compiler; \
    cargo install --version 2.6.0 protobuf-codegen; \
    protoc --rust_out . btt.proto;"

sudo chown -R `whoami` *.rs

mv -f btt.rs ./src/protobuf_structs/btt.rs
