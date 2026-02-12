#!/usr/bin/env bash
set -euo pipefail

CRATES=(
    qorvex-repl
    qorvex-live
    qorvex-cli
    qorvex-auto
)

for crate in "${CRATES[@]}"; do
    echo "Installing ${crate}..."
    cargo install --path "crates/${crate}"
done

echo "All crates installed."

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
