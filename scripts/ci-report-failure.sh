#!/usr/bin/env bash
# Open (or add to) the GitHub issue that says a scheduled workflow failed.
#
# A push or a pull request has an author who is told when its checks fail. A
# scheduled run has nobody: its red mark sits on the Actions tab until someone
# happens to look. Workflows that run on a schedule call this from a step
# guarded by `if: failure() && github.event_name == 'schedule'`, so the failure
# lands where open work is tracked. One open issue per title: a failure that
# repeats nightly adds a comment to it rather than opening another.
#
# usage: ci-report-failure.sh <issue title> [file with details]
# needs: GH_TOKEN with `issues: write`, and the GITHUB_* variables Actions sets.
set -euo pipefail

title="$1"
details="${2:-}"

run_url="${GITHUB_SERVER_URL}/${GITHUB_REPOSITORY}/actions/runs/${GITHUB_RUN_ID}"
body="$(mktemp)"
{
    echo "The scheduled \`${GITHUB_WORKFLOW}\` run failed at ${GITHUB_SHA}: ${run_url}"
    if [ -n "$details" ] && [ -s "$details" ]; then
        echo
        echo '```'
        # GitHub rejects a body over 65,536 characters; the tail is where a
        # tool prints its verdict.
        tail -c 20000 "$details"
        echo '```'
    fi
} > "$body"

existing="$(gh issue list --repo "$GITHUB_REPOSITORY" --state open \
    --search "in:title \"$title\"" --json number,title \
    --jq "map(select(.title == \"$title\")) | .[0].number // empty")"

if [ -n "$existing" ]; then
    gh issue comment "$existing" --repo "$GITHUB_REPOSITORY" --body-file "$body"
else
    gh issue create --repo "$GITHUB_REPOSITORY" --title "$title" --body-file "$body"
fi
