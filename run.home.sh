#!/bin/bash


ROOTDIR=$PWD


"${ANDRUAV_SERVER_BIN:-./target/release/andruavmainserver}" --config=server.home.config


