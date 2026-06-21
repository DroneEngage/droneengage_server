#!/bin/bash

# Parent mode - acts as super server accepting connections from slaves
ROOTDIR=$PWD

cd /home/mhefny/TDisk/public_versions/andruav/andruav_server
node server.js --config=deployment/server.s2s.super.config
