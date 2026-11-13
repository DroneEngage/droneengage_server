#!/bin/bash

# Slave mode - forwards messages to parent server
ROOTDIR=$PWD

cd "$(dirname "$0")/.."
"${ANDRUAV_SERVER_BIN:-./target/release/andruavmainserver}" --config=deployment/server.s2s.drone.config
