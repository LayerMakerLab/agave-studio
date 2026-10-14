#!/bin/sh
# Builds native/speech/agave-speech (needs Xcode's Swift; fetches FluidAudio 0.17.7 the first time).
set -e
cd "$(dirname "$0")"
swift build -c release --product agave-speech
cp "$(swift build -c release --show-bin-path)/agave-speech" ./agave-speech
