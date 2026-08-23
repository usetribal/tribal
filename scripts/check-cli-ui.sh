#!/usr/bin/env bash
# Human-facing `tribal` stdout is owned by crates/lineage-cli/src/ui.rs.
# A new command that println!s its own layout, dumps {:?}, or pulls in a
# second colour crate fights that. Clippy denies print_stdout / use_debug on
# the crate; this script is the part clippy cannot see (which files may print,
# which paint crates are allowed).
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

src="crates/lineage-cli/src"
manifest="crates/lineage-cli/Cargo.toml"
# Wizard boxes stay in init_cmd; everything else prints through ui.
allow_print="ui.rs|init_cmd.rs"

failed=0

# --- println! / print! outside the allowlist --------------------------------

print_hits="$(
  grep -RIn --include='*.rs' -E '\b(println|print)!' "$src" \
    | grep -vE "/($allow_print):" \
    | grep -vE '^\S+:[[:space:]]*//' \
    || true
)"
if [ -n "$print_hits" ]; then
  echo "Human-facing stdout must go through crates/lineage-cli/src/ui.rs"
  echo "(init_cmd.rs wizard boxes are the only other allowed println!)."
  echo
  echo "$print_hits"
  echo
  echo "Use ui::heading / kv / action / indent / empty / hero / print_scan_rows"
  echo "for people, and ui::json / jsonl / raw / raw_line for --json, --discover,"
  echo "context hook, and fork --brief."
  failed=1
fi

# --- Debug format in library/binary sources (not tests) ---------------------

debug_hits="$(
  grep -RIn --include='*.rs' -E '\{\:\?\}' "$src" \
    | grep -vE ':[[:space:]]*(//|//!|\*)' \
    || true
)"
if [ -n "$debug_hits" ]; then
  echo "Do not Debug-format ({:?}) in CLI output. Use ui::role_name,"
  echo "ui::confidence_name, or a lowercase word."
  echo
  echo "$debug_hits"
  failed=1
fi

# --- Second colour / table crate --------------------------------------------

# anstyle + anstream are the clap stack already in the lockfile. A second
# paint crate (owo-colors, colored, …) or a table crate would split the look.
forbidden='owo-colors|colored|comfy-table|tabled|nu-ansi-term|ansi_term'
dep_hits="$(
  grep -nE "^($forbidden)[[:space:]]*=" "$manifest" || true
)"
if [ -n "$dep_hits" ]; then
  echo "lineage-cli paints with anstyle/anstream only (see src/ui.rs)."
  echo "Do not add a second colour or table crate:"
  echo
  echo "$dep_hits"
  failed=1
fi

# --- Old command name -------------------------------------------------------
# The binary is `tribal`. `git lineage` / `git-lineage` only belong in
# migrate.rs, which rewrites leftover hooks and installed skills.
old_name='git lineage|git-lineage'
name_hits="$(
  {
    grep -RIn --include='*.rs' -E "$old_name" "$src" \
      | grep -vE '/migrate\.rs:' \
      || true
    grep -nE "$old_name" "$manifest" || true
    grep -RIn -E "$old_name" "$root/crates/lineage-cli/assets" || true
  }
)"
if [ -n "$name_hits" ]; then
  echo "The CLI command is tribal, not git lineage / git-lineage."
  echo "Leave the old name only in migrate.rs (it rewrites leftover copies)."
  echo
  echo "$name_hits"
  failed=1
fi

if [ "$failed" -ne 0 ]; then
  exit 1
fi

echo "CLI presentation lint passed."
