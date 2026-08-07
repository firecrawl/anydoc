#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd -P)"
private_markdown="$repo_root/henry-mvp/out/private/blacklake.md"
cd "$repo_root"

if [[ ! -s "$private_markdown" ]]; then
  echo "privacy audit requires the generated private Markdown" >&2
  exit 1
fi

first_mvp_commit="$(git rev-list --reverse HEAD -- henry-mvp | sed -n '1p')"
if [[ -z "$first_mvp_commit" ]]; then
  echo "privacy audit found no MVP commits" >&2
  exit 1
fi

# Construct the forbidden original-source location without storing that private
# absolute path in this tracked script.
desktop_pattern='/Users/''[^/][^/]*/''Desk''top/'
commit_count=0
while IFS= read -r commit; do
  [[ -n "$commit" ]] || continue
  commit_count=$((commit_count + 1))

  tracked_paths="$(git ls-tree -r --name-only "$commit")"
  if rg -q '(^|/)(source\.pdf|blacklake\.md|report\.json)$|(^|/)out/private(/|$)' \
    <<<"$tracked_paths"; then
    echo "privacy audit failed: private artifact path tracked in commit $commit" >&2
    exit 1
  fi

  if git grep -I -q -E "$desktop_pattern" "$commit"; then
    echo "privacy audit failed: original-source path indicator in commit $commit" >&2
    exit 1
  fi

  while IFS= read -r private_line || [[ -n "$private_line" ]]; do
    if (( ${#private_line} < 12 )); then
      continue
    fi
    if git grep -I -F -q -e "$private_line" "$commit"; then
      echo "privacy audit failed: private-content overlap in commit $commit" >&2
      exit 1
    fi
  done <"$private_markdown"
done < <(git rev-list --reverse "${first_mvp_commit}^..HEAD")

echo "privacy_audit_ok commits=$commit_count"
