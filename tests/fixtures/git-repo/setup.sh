#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")" && pwd)"
cd "$ROOT"
rm -rf .git
git init -q
git config user.email "lineage@test.dev"
git config user.name "Lineage Test"
echo 'fn main() { println!("hello"); }' > main.rs
git add main.rs
git commit -q -m "initial commit"
