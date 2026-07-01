#!/usr/bin/env bash
#
# Pre-flight gate: fail if the gcloud upload set for DIR meets or exceeds
# App Engine Standard's hard per-deploy file cap (10,000 files).
#
# Why this exists (issue #695): `gcloud app deploy` uploads everything that
# .gcloudignore does not exclude, which is INDEPENDENT of git tracking
# status. A `git status`-clean tree can still hold gitignored artifacts
# (cargo target dirs, Python venvs, third_party checkouts) or brand-new
# untracked scratch dirs that blow the cap -- and without this gate the
# failure only surfaces inside `gcloud app deploy`, after the expensive
# clean+build. `gcloud meta list-files-for-upload` is the only check that
# reflects the real upload set, so we run it once here, immediately before
# the deploy, and turn a late upload failure into a fast, actionable error.
#
# Usage: check-upload-file-count.sh DIR [MAX_FILES]
#   MAX_FILES defaults to 10000 (the GAE Standard cap). It is overridable
#   so the failure path can be exercised by hand against a small directory
#   without building a 10k-file fixture.

set -euo pipefail

if [ "$#" -lt 1 ] || [ "$#" -gt 2 ]; then
    echo "usage: $0 DIR [MAX_FILES]" >&2
    exit 2
fi

DIR="$1"
MAX_FILES="${2:-10000}"

UPLOAD_LIST="$(mktemp)"
trap 'rm -f "$UPLOAD_LIST"' EXIT

# Enumerate exactly once: on a polluted tree this walk covers 100k+ files
# and is the slow part, so both the count and the per-directory breakdown
# below are derived from this single capture. Running from inside DIR makes
# gcloud emit paths relative to DIR, which the breakdown's `cut` relies on.
(cd "$DIR" && gcloud meta list-files-for-upload .) > "$UPLOAD_LIST"

UPLOAD_COUNT="$(wc -l < "$UPLOAD_LIST" | tr -d '[:space:]')"

if [ "$UPLOAD_COUNT" -ge "$MAX_FILES" ]; then
    {
        echo "ERROR: gcloud would upload $UPLOAD_COUNT files from $DIR, but App Engine"
        echo "       Standard rejects deploys of $MAX_FILES files or more."
        echo ""
        echo "The upload set is whatever .gcloudignore leaves in -- a clean 'git status'"
        echo "does NOT bound it (gitignored and untracked files still upload). Largest"
        echo "top-level directories in the upload set; delete the junk ones or add them"
        echo "to .gcloudignore:"
        echo ""
        cut -d/ -f1 "$UPLOAD_LIST" | sort | uniq -c | sort -rn | head -10
    } >&2
    exit 1
fi

echo "    upload set: $UPLOAD_COUNT files (cap: $MAX_FILES)"
