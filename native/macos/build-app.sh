#!/usr/bin/env bash
# Builds the AgaveStudio SwiftPM executable and assembles build/Agave Studio.app (ad-hoc signed).
# Not notarized, not installed to /Applications.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$HERE"

swift build -c release
BIN_DIR="$(swift build -c release --show-bin-path)"
EXE="$BIN_DIR/AgaveStudio"
[[ -x "$EXE" ]] || { echo "error: missing executable at $EXE" >&2; exit 1; }

APP="$HERE/build/Agave Studio.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$EXE" "$APP/Contents/MacOS/AgaveStudio"
cp "$HERE/AppIcon.icns" "$APP/Contents/Resources/AppIcon.icns"
chmod 755 "$APP/Contents/MacOS/AgaveStudio"

cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key>
    <string>en</string>
    <key>CFBundleIconFile</key>
    <string>AppIcon</string>
    <key>CFBundleExecutable</key>
    <string>AgaveStudio</string>
    <key>CFBundleIdentifier</key>
    <string>local.agavestudio.app</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleName</key>
    <string>Agave Studio</string>
    <key>CFBundleDisplayName</key>
    <string>Agave Studio</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>0.1.0</string>
    <key>CFBundleVersion</key>
    <string>1</string>
    <key>LSMinimumSystemVersion</key>
    <string>14.0</string>
    <key>LSApplicationCategoryType</key>
    <string>public.app-category.video</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSPrincipalClass</key>
    <string>NSApplication</string>
    <key>NSAppTransportSecurity</key>
    <dict>
        <key>NSAllowsLocalNetworking</key>
        <true/>
        <key>NSExceptionDomains</key>
        <dict>
            <key>127.0.0.1</key>
            <dict>
                <key>NSExceptionAllowsInsecureHTTPLoads</key>
                <true/>
            </dict>
            <key>localhost</key>
            <dict>
                <key>NSExceptionAllowsInsecureHTTPLoads</key>
                <true/>
            </dict>
        </dict>
    </dict>
</dict>
</plist>
PLIST
# The app starts the gateway from the checkout it was built in.
# plutil takes the path as its own argument, so quotes or spaces in it are safe (PlistBuddy's command string is not).
plutil -insert AgaveRepo -string "$(cd "$HERE/../.." && pwd)" "$APP/Contents/Info.plist"

codesign --force -s - "$APP"
codesign --verify --verbose=2 "$APP"
echo "Built: $APP"
