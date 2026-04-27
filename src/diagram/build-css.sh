#!/usr/bin/env bash
set -euo pipefail

# Generate per-CSS-file outputs for the @simlin/diagram build.
#
# - `lib/<rel>/<file>.css` is a JavaScript stub exporting an empty proxy.
#   The Node build (consumed by simlin-cli, simlin-serve's MCP layer, and
#   any non-bundler user) imports `.css` modules through the package's
#   `./*.css` export. Without a stub the import resolves to a literal CSS
#   file that Node cannot evaluate; the proxy makes
#   `import styles from "./foo.css"` return an object whose every property
#   is the empty string, so consumers fall back to whatever class names
#   the bundler (in browser builds) would produce.
# - `lib.browser/<rel>/<file>.css` is a copy of the source CSS so the
#   browser bundle (consumed by Vite / esbuild / etc.) can apply real
#   styles.
#
# Inlined as a bash script so the build runs identically on Linux,
# macOS, and Windows. The previous in-package.json shell pipeline
# relied on `find`/`while`/`dirname`/`sed` being on PATH, which fails
# on the Windows GitHub runner where pnpm shells out through PowerShell.
# `bash <script>` runs through Git Bash on Windows and the system bash
# on Unix.

DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" >/dev/null 2>&1 && pwd )"
cd "$DIR"

find . -name '*.css' -not -path './lib*' -not -path './node_modules/*' | while read -r src; do
  rel_dir=$(dirname "$src" | sed 's|^\./||')

  mkdir -p "lib/$rel_dir"
  printf 'module.exports = new Proxy({}, { get: () => "" });\n' \
    > "lib/$rel_dir/$(basename "$src")"

  mkdir -p "lib.browser/$rel_dir"
  cp "$src" "lib.browser/$rel_dir/"
done
