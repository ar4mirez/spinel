#!/usr/bin/env bash
# Fails when a PR title has an `@identifier` outside backticks. GitHub reads a
# bare `@x` as a user mention, and generated release notes then credit (and
# ping) that user. Usage: scripts/check-pr-title.sh "<title>" (or $PR_TITLE).
set -euo pipefail
title="${1-${PR_TITLE-}}"
# Drop backtick code spans, then look for `@` starting a word, as GitHub does.
bare="$(printf '%s' "$title" | sed 's/`[^`]*`//g')"
if printf '%s' "$bare" | grep -Eq '(^|[^[:alnum:]_.-])@[[:alnum:]_]'; then
  found="$(printf '%s' "$bare" | grep -Eo '(^|[^[:alnum:]_.-])@[[:alnum:]_-]+' | sed 's/^[^@]*//' | sort -u | tr '\n' ' ')"
  echo "::error title=PR title has a bare @-identifier::Wrap ${found}in backticks (\`@x\`): GitHub treats a bare @name as a user mention, and release notes are built from PR titles."
  exit 1
fi
echo "PR title OK"
