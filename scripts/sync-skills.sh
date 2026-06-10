#!/usr/bin/env bash
# Copy project skills from .cursor/skills/ to .agents/skills/ and .claude/skills/.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC="${ROOT}/.cursor/skills"

if [[ ! -d "${SRC}" ]]; then
  echo "error: ${SRC} not found" >&2
  exit 1
fi

for skill_dir in "${SRC}"/*/; do
  name="$(basename "${skill_dir}")"
  if [[ ! -f "${skill_dir}/SKILL.md" ]]; then
    continue
  fi
  for dest in .agents/skills .claude/skills; do
    mkdir -p "${ROOT}/${dest}/${name}"
    cp "${skill_dir}/SKILL.md" "${ROOT}/${dest}/${name}/SKILL.md"
  done
  echo "synced ${name}"
done
