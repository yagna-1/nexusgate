#!/usr/bin/env bash
set -euo pipefail

if [[ -f RULES.md && ! -s RULES.md ]]; then
  echo "RULES.md is empty" >&2
  exit 1
fi

echo "pre-run checks passed"
