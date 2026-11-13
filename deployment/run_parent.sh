#!/bin/bash

# Parent mode - acts as super server accepting connections from slaves
ROOTDIR=$PWD

cd "$(dirname "$0")/.."
"${ANDRUAV_SERVER_BIN:-./target/release/andruavmainserver}" --config=deployment/server.s2s.super.config
