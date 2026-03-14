#!/usr/bin/env bash

set -euo pipefail

pattern='[A-Z]+-[0-9]+'
detected=false
source="none"
feature_prefix="feature"

if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  refs="$(git for-each-ref --format='%(refname:short)' refs/heads refs/remotes 2>/dev/null || true)"

  feat_count="$(printf '%s\n' "$refs" | rg -c '^feat/' || true)"
  feature_count="$(printf '%s\n' "$refs" | rg -c '^feature/' || true)"
  if [ "${feat_count:-0}" -gt "${feature_count:-0}" ]; then
    feature_prefix="feat"
  fi

  if printf '%s\n' "$refs" | rg -q "$pattern"; then
    detected=true
    source="branches"
  elif git log --format='%s%n%b' -n 200 2>/dev/null | rg -q "$pattern"; then
    detected=true
    source="history"
  fi
else
  echo "error=not-a-git-repository"
  exit 1
fi

echo "detected=$detected"
echo "source=$source"
echo "feature_prefix=$feature_prefix"
