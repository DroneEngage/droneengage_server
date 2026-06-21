#!/bin/bash

# Slave mode - forwards messages to parent server
ROOTDIR=$PWD

cd /home/mhefny/TDisk/public_versions/andruav/andruav_server
node server.js --config=deployment/server.s2s.drone.config
