#!/usr/bin/env bash
set -euo pipefail

MSRV="${MSRV:-1.86.0}"

if ! rustup toolchain list | grep -q "^${MSRV}-"; then
  echo "Installing Rust ${MSRV} for MSRV check..."
  rustup toolchain install "${MSRV}" --profile minimal
fi

echo "MSRV check on Rust ${MSRV}"
cargo +"${MSRV}" test --workspace
