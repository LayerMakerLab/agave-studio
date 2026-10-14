#!/bin/sh
# Builds agave-vision next to this script (macOS only; needs the Xcode command line tools).
set -e
cd "$(dirname "$0")"
swiftc -O -o agave-vision main.swift
echo "built $(pwd)/agave-vision"
