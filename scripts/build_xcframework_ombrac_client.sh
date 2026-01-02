#!/bin/bash

set -e

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" &> /dev/null && pwd)
PROJECT_ROOT=$(cd "$SCRIPT_DIR/.." && pwd)

CRATE_NAME="ombrac-client"
FRAMEWORK_NAME="OmbracClient"
HEADER_NAME="ombrac_client.h"

LIB_NAME="${CRATE_NAME//-/_}"
CRATE_PATH="$PROJECT_ROOT/crates/$CRATE_NAME"
OUTPUT_DIR="$PROJECT_ROOT/target/xcframework"
BUILD_DIR="$PROJECT_ROOT/target/xcframework_build"
HEADER_PATH="$CRATE_PATH/$HEADER_NAME"

cd "$PROJECT_ROOT"

echo "🚀 Starting XCFramework build for $FRAMEWORK_NAME..."
echo "Project Root: $PROJECT_ROOT"

rm -rf "$BUILD_DIR"
rm -rf "$OUTPUT_DIR/$FRAMEWORK_NAME.xcframework"
mkdir -p "$BUILD_DIR"
mkdir -p "$OUTPUT_DIR"


IOS_ARM64="aarch64-apple-ios"
SIM_ARM64="aarch64-apple-ios-sim"
SIM_X86_64="x86_64-apple-ios"
MACOS_ARM64="aarch64-apple-darwin"
MACOS_X86_64="x86_64-apple-darwin"

TARGETS=($IOS_ARM64 $SIM_ARM64 $SIM_X86_64 $MACOS_ARM64 $MACOS_X86_64)

for TARGET in "${TARGETS[@]}"; do
    echo "Building for $TARGET..."
    rustup target add $TARGET
    cargo build --release --features ffi --target $TARGET -p $CRATE_NAME
    mkdir -p "$BUILD_DIR/$TARGET"
    cp "target/$TARGET/release/lib${LIB_NAME}.a" "$BUILD_DIR/$TARGET/"
done

echo "Creating universal library for iOS simulators..."
mkdir -p "$BUILD_DIR/ios-sim-universal"
lipo -create \
    "$BUILD_DIR/$SIM_ARM64/lib${LIB_NAME}.a" \
    "$BUILD_DIR/$SIM_X86_64/lib${LIB_NAME}.a" \
    -output "$BUILD_DIR/ios-sim-universal/lib${LIB_NAME}.a"

echo "Creating universal library for macOS..."
mkdir -p "$BUILD_DIR/macos-universal"
lipo -create \
    "$BUILD_DIR/$MACOS_ARM64/lib${LIB_NAME}.a" \
    "$BUILD_DIR/$MACOS_X86_64/lib${LIB_NAME}.a" \
    -output "$BUILD_DIR/macos-universal/lib${LIB_NAME}.a"

echo "Preparing header files and module map..."

HEADERS_DIR="$BUILD_DIR/headers"
mkdir -p "$HEADERS_DIR"

if [ ! -f "$HEADER_PATH" ]; then
    echo "❌ Header file not found at $HEADER_PATH"
    exit 1
fi
cp "$HEADER_PATH" "$HEADERS_DIR/"

cat > "$HEADERS_DIR/module.modulemap" <<EOL
module $FRAMEWORK_NAME {
    umbrella header "$HEADER_NAME"
    export *
}
EOL

echo "Creating XCFramework for iOS and macOS..."
xcodebuild -create-xcframework \
    -library "$BUILD_DIR/$IOS_ARM64/lib${LIB_NAME}.a" -headers "$HEADERS_DIR" \
    -library "$BUILD_DIR/ios-sim-universal/lib${LIB_NAME}.a" -headers "$HEADERS_DIR" \
    -library "$BUILD_DIR/macos-universal/lib${LIB_NAME}.a" -headers "$HEADERS_DIR" \
    -output "$OUTPUT_DIR/$FRAMEWORK_NAME.xcframework"


rm -rf "$BUILD_DIR"

echo ""
echo "✅ Successfully created $OUTPUT_DIR/$FRAMEWORK_NAME.xcframework"
