#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
#
# Repository audit, run by the pre-commit hook and by CI.
#   tools/audit.sh            check all tracked files
#   tools/audit.sh --staged   check the files staged for commit (pre-commit hook)
#
# Checks: no game files or generated code, no large files, no C or C++ sources (the project
# is Rust only), SPDX headers on sources, no absolute home paths.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

mode=${1:-all}
case "$mode" in
  all) mapfile -t files < <(git ls-files) ;;
  --staged) mapfile -t files < <(git diff --cached --name-only --diff-filter=ACMR) ;;
  *) echo "usage: $0 [--staged]" >&2; exit 2 ;;
esac

fail=0
err() { echo "audit: $*" >&2; fail=1; }

# Game file types that must never be committed, whatever their path.
forbidden_ext='exe|dll|m3d|ovl|iso|bin|cue|cab|hdr|inx|rar|bik|pck|tpl|cmp|lvl|sbk|rul|dlg|pal|wav|mp3|zip|pdf|upx'
max_size=$((512 * 1024))

for f in "${files[@]}"; do
  [[ -f "$f" ]] || continue
  case "$f" in
    orig/*|generated/*|build/*|*/target/*) err "$f: inside a git-ignored area (game files / build output)"; continue ;;
  esac
  ext=${f##*.}
  ext=${ext,,}
  if [[ "$f" == *.* && "$ext" =~ ^($forbidden_ext)$ ]]; then
    err "$f: file type .$ext is not allowed (game data or binaries)"
  fi
  if [[ "$ext" =~ ^(c|h|cc|cpp|cxx|hpp|hh|m|mm|s|asm)$ ]]; then
    err "$f: C, C++ or assembly source (the project is Rust only, docs/PLAN.md decision log)"
  fi
  size=$(wc -c < "$f")
  if (( size > max_size )); then
    err "$f: $size bytes (limit $max_size)"
  fi

  needs_spdx=0
  case "$f" in
    *.rs|*.toml|*.sh|*.py|*.yml|*.yaml) needs_spdx=1 ;;
    tools/hooks/*) needs_spdx=1 ;;
  esac
  # Vendored code keeps its own licence header (third_party/README.md lists each).
  case "$f" in
    third_party/*) needs_spdx=0 ;;
  esac
  if (( needs_spdx )) && ! head -n 5 "$f" | grep -q 'SPDX-License-Identifier: MIT'; then
    err "$f: missing 'SPDX-License-Identifier: MIT' in the first 5 lines"
  fi

  # Absolute paths of a developer machine. This script is exempt (it contains the patterns).
  if [[ "$f" != tools/audit.sh ]] && grep -IqE '/home/[a-z][a-z0-9_-]*/|/Users/[A-Za-z][A-Za-z0-9_-]*/|[Cc]:\\Users\\' "$f"; then
    err "$f: contains an absolute home directory path"
  fi
done

if (( fail )); then
  echo "audit: FAILED (see CONTRIBUTING.md)" >&2
  exit 1
fi
echo "audit: ${#files[@]} files ok"
