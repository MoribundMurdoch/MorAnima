#!/bin/sh
# Build the MorAnima Android APK with raw SDK build-tools (no gradle).
# Usage: KEYSTORE=~/Android/moranima.keystore KS_PASS=... ./build-apk.sh
set -e
cd "$(dirname "$0")"
SDK="${ANDROID_HOME:-$HOME/Android/Sdk}"
BT="$SDK/build-tools/36.0.0"
PLATFORM="$SDK/platforms/android-35/android.jar"

rm -rf build assets && mkdir -p build/classes assets
cp ../docs/index.html assets/index.html

javac --release 11 -classpath "$PLATFORM" -d build/classes \
  java/com/moribund/moranima/MainActivity.java
"$BT/d8" --release --min-api 29 --lib "$PLATFORM" --output build \
  build/classes/com/moribund/moranima/*.class
"$BT/aapt" package -f -M AndroidManifest.xml -S res -A assets \
  -I "$PLATFORM" -F build/unsigned.apk
(cd build && "$BT/aapt" add unsigned.apk classes.dex)
"$BT/zipalign" -f 4 build/unsigned.apk build/aligned.apk
"$BT/apksigner" sign --ks "$KEYSTORE" --ks-pass "pass:$KS_PASS" \
  --out build/moranima.apk build/aligned.apk
echo "Built: $(pwd)/build/moranima.apk"
