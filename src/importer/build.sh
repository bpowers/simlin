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

rm -rf lib lib.browser

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
PKG_NAME=${PWD##*/}

mv pkg core

yarn run tsc
yarn run tsc -p tsconfig.browser.json

rm -r lib/core lib/pkg-node
rm -r lib.browser/core lib.browser/pkg-node

cp -r pkg-node lib/core
cp -r core lib.browser/

rm -r pkg-node

mv lib/index{_main,}.js
mv lib/index{_main,}.js.map
mv lib/index{_main,}.d.ts
rm lib.browser/index_main*
