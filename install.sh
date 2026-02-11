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
