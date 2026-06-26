#!/usr/bin/env bash
set -euo pipefail

CRATES=(
    qorvex-server
    qorvex-repl
    qorvex-live
    qorvex-cli
)

for crate in "${CRATES[@]}"; do
    echo "Installing ${crate}..."
    cargo install --path "crates/${crate}"
done

echo "All crates installed."

# Build and install Swift streamer (macOS only)
if [[ "$(uname)" == "Darwin" ]]; then
    echo "Building qorvex-streamer..."
    make -C qorvex-streamer build
    CARGO_BIN="${CARGO_HOME:-$HOME/.cargo}/bin"
    if [ -d "$CARGO_BIN" ]; then
        cp qorvex-streamer/.build/release/qorvex-streamer "$CARGO_BIN/"
        echo "qorvex-streamer installed to $CARGO_BIN/"
    else
        echo "Warning: Could not find $CARGO_BIN — install qorvex-streamer manually"
    fi
fi

# Record agent source directories in config (iOS + Android)
REPO_ROOT="$(cd "$(dirname "$0")" && pwd)"
AGENT_SOURCE_DIR="$REPO_ROOT/qorvex-agent"
ANDROID_AGENT_SOURCE_DIR="$REPO_ROOT/qorvex-agent-android"
CONFIG_DIR="$HOME/.qorvex"
CONFIG_FILE="$CONFIG_DIR/config.json"

mkdir -p "$CONFIG_DIR"

if [ -f "$CONFIG_FILE" ]; then
    python3 -c "
import json, sys
ios_path, android_path = sys.argv[1], sys.argv[2]
with open(sys.argv[3], 'r') as f:
    config = json.load(f)
config['agent_source_dir'] = ios_path
config['android_agent_source_dir'] = android_path
with open(sys.argv[3], 'w') as f:
    json.dump(config, f, indent=2)
    f.write('\n')
" "$AGENT_SOURCE_DIR" "$ANDROID_AGENT_SOURCE_DIR" "$CONFIG_FILE"
else
    printf '{\n  "agent_source_dir": "%s",\n  "android_agent_source_dir": "%s"\n}\n' \
        "$AGENT_SOURCE_DIR" "$ANDROID_AGENT_SOURCE_DIR" > "$CONFIG_FILE"
fi

echo "Agent source recorded: $AGENT_SOURCE_DIR"
echo "Android agent source recorded: $ANDROID_AGENT_SOURCE_DIR"

# Build agent for simulator (generic destination, no specific UDID needed)
echo "Building qorvex-agent for simulator..."
xcodebuild build-for-testing \
    -project "$AGENT_SOURCE_DIR/QorvexAgent.xcodeproj" \
    -scheme QorvexAgentUITests \
    -destination "generic/platform=iOS Simulator" \
    -derivedDataPath "$AGENT_SOURCE_DIR/.build" \
    -quiet
echo "qorvex-agent built (simulator)."

# Build agent for physical devices
echo "Building qorvex-agent for physical devices..."
xcodebuild build-for-testing \
    -project "$AGENT_SOURCE_DIR/QorvexAgent.xcodeproj" \
    -scheme QorvexAgentUITests \
    -destination "generic/platform=iOS" \
    -derivedDataPath "$AGENT_SOURCE_DIR/.build" \
    -quiet
echo "qorvex-agent built (physical)."

# Pre-build the Android agent APKs (best-effort: needs a JDK + Android SDK).
# Unlike Xcode, the Android SDK is not a guaranteed dependency, so a missing
# toolchain warns instead of failing the install — the agent still builds on
# first use once the SDK is present.
if [ -x "$ANDROID_AGENT_SOURCE_DIR/gradlew" ]; then
    if command -v adb >/dev/null 2>&1 \
        || [ -n "${ANDROID_HOME:-}" ] \
        || [ -n "${ANDROID_SDK_ROOT:-}" ] \
        || [ -f "$ANDROID_AGENT_SOURCE_DIR/local.properties" ]; then
        echo "Building qorvex-agent-android APKs..."
        if (cd "$ANDROID_AGENT_SOURCE_DIR" && ./gradlew assembleDebug assembleDebugAndroidTest); then
            echo "qorvex-agent-android built."
        else
            echo "Warning: qorvex-agent-android build failed — it will build on first use."
        fi
    else
        echo "Skipping qorvex-agent-android pre-build: no Android SDK found (set ANDROID_HOME or put adb on PATH). It will build on first use."
    fi
fi
