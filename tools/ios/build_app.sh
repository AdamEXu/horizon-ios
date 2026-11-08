#!/bin/sh
# Builds Horizon.app for the iOS Simulator (default) or a device, and installs it on a
# simulator when asked. Dev tool; the resources are the ones horizon-prepare made from the
# player's own Minecraft (.local/horizon-client/26.3), never committed.
#
#   tools/ios/build_app.sh                     # simulator build, app in target/ios/Horizon.app
#   tools/ios/build_app.sh --install UDID      # and install it on that simulator
#   tools/ios/build_app.sh --device --sign "Apple Development: ..." --profile path.mobileprovision
#
# Env: PROFILE (cargo profile, default fast), RESOURCES (default .local/horizon-client/26.3),
#      BUNDLE_ID (default dev.horizonlab.horizon.ios), NO_SOUNDS=1 to leave the sounds out.
set -eu
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
cd "$ROOT"
PROFILE=${PROFILE:-fast}
RESOURCES=${RESOURCES:-$ROOT/.local/horizon-client/26.3}
BUNDLE_ID=${BUNDLE_ID:-dev.horizonlab.horizon.ios}
TARGET=aarch64-apple-ios-sim
INSTALL=""
SIGN="-"
PROVISION=""
while [ $# -gt 0 ]; do
    case "$1" in
        --device) TARGET=aarch64-apple-ios ;;
        --install) INSTALL=$2; shift ;;
        --sign) SIGN=$2; shift ;;
        --profile) PROVISION=$2; shift ;;
        *) echo "unknown argument $1" >&2; exit 2 ;;
    esac
    shift
done
export PATH="$HOME/.cargo/bin:$PATH"
if [ "$TARGET" = aarch64-apple-ios ]; then
    export IPHONEOS_DEPLOYMENT_TARGET=${IPHONEOS_DEPLOYMENT_TARGET:-17.0}
    MIN_KEY=iphoneos
else
    export IPHONEOS_DEPLOYMENT_TARGET=${IPHONEOS_DEPLOYMENT_TARGET:-17.0}
    MIN_KEY=iphonesimulator
fi
cargo build --profile "$PROFILE" --target "$TARGET" -p horizon-ios

APP="$ROOT/target/ios/$TARGET/Horizon.app"
mkdir -p "$APP"
cp "target/$TARGET/$PROFILE/horizon-ios" "$APP/horizon-ios"
sed -e "s/@BUNDLE_ID@/$BUNDLE_ID/" -e "s/@MIN_OS@/$IPHONEOS_DEPLOYMENT_TARGET/" \
    -e "s/@PLATFORM@/$( [ "$MIN_KEY" = iphoneos ] && echo iPhoneOS || echo iPhoneSimulator )/" \
    "$ROOT/crates/horizon-ios/Info.plist" > "$APP/Info.plist"
EXCLUDE=""
[ "${NO_SOUNDS:-0}" = 1 ] && EXCLUDE="--exclude=sounds/"
# shellcheck disable=SC2086
rsync -a --delete $EXCLUDE "$RESOURCES/" "$APP/GameData/"
if [ -n "$PROVISION" ]; then
    cp "$PROVISION" "$APP/embedded.mobileprovision"
fi
codesign --force --sign "$SIGN" --timestamp=none "$APP"
echo "built $APP"
if [ -n "$INSTALL" ]; then
    xcrun simctl install "$INSTALL" "$APP"
    echo "installed $BUNDLE_ID on $INSTALL"
fi
