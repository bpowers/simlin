#!/usr/bin/env bash
set -euo pipefail

DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" >/dev/null 2>&1 && pwd )"
cd "$DIR"

# Check if jq is installed
if ! [ -x "$(command -v jq)" ]; then
    echo "jq is not installed" >& 2
    exit 1
fi

# Clean previous packages
if [ -d "pkg" ]; then
    rm -rf pkg
fi

if [ -d "pkg-node" ]; then
    rm -rf pkg-node
fi

if [ -d "core" ]; then
    rm -rf core
fi

# Build for both targets
CC=emcc CXX=em++ wasm-pack build --release -t nodejs -d pkg-node
CC=emcc CXX=em++ wasm-pack build --release -t browser -d pkg

rm pkg/package.json
rm pkg/.gitignore

#wasm-opt pkg/engine_v2_bg.wasm -o pkg/engine_v2_bg.wasm-opt.wasm -O2 --enable-mutable-globals
#wasm-opt pkg-node/engine_v2_bg.wasm -o pkg-node/engine_v2_bg.wasm-opt.wasm -O2 --enable-mutable-globals
#
#mv pkg/engine_v2_bg.{wasm-opt.,}wasm
#mv pkg-node/engine_v2_bg.{wasm-opt.,}wasm

# Get the package name
PKG_NAME=importer

# Merge nodejs & browser packages
cp "pkg-node/${PKG_NAME}.js" "pkg/${PKG_NAME}_main.js"

rm -rf pkg-node

mv pkg core

yarn run tsc
yarn run tsc -p tsconfig.browser.json

mv lib/importer/index* lib/
mv lib.browser/importer/index* lib.browser/

rm -r lib/importer
rm -r lib.browser/importer

cp -r core lib/
cp -r core lib.browser/
mv lib/index_main.js lib/index.js
mv lib/index_main.js.map lib/index.js.map
mv lib/index_main.d.ts lib/index.d.ts
rm lib.browser/index_main*
