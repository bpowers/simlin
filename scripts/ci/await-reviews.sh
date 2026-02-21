#!/usr/bin/env bash
set -euo pipefail

# Usage: scripts/ci/await-reviews.sh <PR_NUMBER>
#
# Polls GitHub until both automated reviewers (claude[bot] and
# chatgpt-codex-connector[bot]) have posted new feedback on the given PR
# since the last recorded timestamp.
#
# On success, prints the OLD timestamp to stdout (for use with gh --jq
# filtering) and writes the current time to .review-timestamp.
#
# Exit codes: 0 = both reviewers posted, 1 = timeout or error.

if [ $# -lt 1 ]; then
  echo "Usage: $0 <PR_NUMBER>" >&2
  exit 1
fi

PR_NUMBER="$1"
REPO_ROOT="$(git rev-parse --show-toplevel)"
TIMESTAMP_FILE="$REPO_ROOT/.review-timestamp"
TIMEOUT_SECONDS=1740  # 29 minutes

# Read last timestamp (or use epoch if no file)
if [ -f "$TIMESTAMP_FILE" ]; then
  SINCE=$(cat "$TIMESTAMP_FILE")
else
  SINCE="1970-01-01T00:00:00Z"
fi

# Resolve owner/repo
REPO=$(gh repo view --json nameWithOwner --jq '.nameWithOwner')

echo "Waiting for reviews on PR #${PR_NUMBER} (since ${SINCE})..." >&2

START_TIME=$(date +%s)

while true; do
  # Check for claude[bot] issue comment newer than $SINCE
  CLAUDE_COUNT=$(gh api "repos/${REPO}/issues/${PR_NUMBER}/comments?since=${SINCE}" \
    --jq '[.[] | select(.user.login == "claude[bot]")] | length' 2>/dev/null || echo "0")

  # Check for codex review newer than $SINCE
  # The reviews endpoint doesn't support ?since=, so we filter client-side.
  CODEX_COUNT=$(gh api "repos/${REPO}/pulls/${PR_NUMBER}/reviews" \
    --jq '[.[] | select(.user.login == "chatgpt-codex-connector[bot]" and .submitted_at > "'"${SINCE}"'")] | length' 2>/dev/null || echo "0")

  if [ "$CLAUDE_COUNT" -gt 0 ] && [ "$CODEX_COUNT" -gt 0 ]; then
    break
  fi

  # Check timeout
  ELAPSED=$(( $(date +%s) - START_TIME ))
  if [ "$ELAPSED" -ge "$TIMEOUT_SECONDS" ]; then
    echo "" >&2
    echo "Timed out after ${TIMEOUT_SECONDS}s waiting for reviews." >&2
    echo "  claude[bot] comments since ${SINCE}: ${CLAUDE_COUNT}" >&2
    echo "  codex reviews since ${SINCE}: ${CODEX_COUNT}" >&2
    exit 1
  fi

  # Progress indicator
  WAITING_FOR=""
  if [ "$CLAUDE_COUNT" -eq 0 ]; then WAITING_FOR="claude"; fi
  if [ "$CODEX_COUNT" -eq 0 ]; then
    if [ -n "$WAITING_FOR" ]; then WAITING_FOR="${WAITING_FOR}, codex"; else WAITING_FOR="codex"; fi
  fi
  echo "  waiting for: ${WAITING_FOR} (${ELAPSED}s elapsed)" >&2

  sleep 15
done

echo "Both reviewers have posted." >&2

# Update timestamp file to current time
date -u +%Y-%m-%dT%H:%M:%SZ > "$TIMESTAMP_FILE"

# Output the OLD timestamp for the caller to use with gh filtering
echo "$SINCE"
