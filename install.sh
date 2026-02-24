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

# Record agent source directory in config
AGENT_SOURCE_DIR="$(cd "$(dirname "$0")" && pwd)/qorvex-agent"
CONFIG_DIR="$HOME/.qorvex"
CONFIG_FILE="$CONFIG_DIR/config.json"

mkdir -p "$CONFIG_DIR"

if [ -f "$CONFIG_FILE" ]; then
    python3 -c "
import json, sys
path = sys.argv[1]
with open(sys.argv[2], 'r') as f:
    config = json.load(f)
config['agent_source_dir'] = path
with open(sys.argv[2], 'w') as f:
    json.dump(config, f, indent=2)
    f.write('\n')
" "$AGENT_SOURCE_DIR" "$CONFIG_FILE"
else
    printf '{\n  "agent_source_dir": "%s"\n}\n' "$AGENT_SOURCE_DIR" > "$CONFIG_FILE"
fi

echo "Agent source recorded: $AGENT_SOURCE_DIR"

# Build agent for simulator (generic destination, no specific UDID needed)
echo "Building qorvex-agent..."
xcodebuild build-for-testing \
    -project "$AGENT_SOURCE_DIR/QorvexAgent.xcodeproj" \
    -scheme QorvexAgentUITests \
    -destination "generic/platform=iOS Simulator" \
    -derivedDataPath "$AGENT_SOURCE_DIR/.build" \
    -quiet
echo "qorvex-agent built."
