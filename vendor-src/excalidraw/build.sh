#!/bin/sh
# Rebuilds web/vendor/excalidraw/ from the exact versions in package-lock.json (needs Node 20 or newer and npm).
# Install scripts never run. The result is the same files, byte for byte; compare with web/vendor/excalidraw/SHA256SUMS.
set -eu
cd "$(dirname "$0")"
npm ci --ignore-scripts --no-audit --no-fund
node build.mjs
