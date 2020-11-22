#!/usr/bin/env bash
set -euo pipefail

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

# Build for both targets
CC=emcc CXX=em++ wasm-pack build --release -t nodejs -d pkg-node
CC=emcc CXX=em++ wasm-pack build --release -t browser -d pkg

#wasm-opt pkg/engine_v2_bg.wasm -o pkg/engine_v2_bg.wasm-opt.wasm -O2 --enable-mutable-globals
#wasm-opt pkg-node/engine_v2_bg.wasm -o pkg-node/engine_v2_bg.wasm-opt.wasm -O2 --enable-mutable-globals
#
#mv pkg/engine_v2_bg.{wasm-opt.,}wasm
#mv pkg-node/engine_v2_bg.{wasm-opt.,}wasm

# Get the package name
PKG_NAME=$(jq -r .name pkg/package.json | sed 's/\-/_/g')

# Merge nodejs & browser packages
cp "pkg-node/${PKG_NAME}.js" "pkg/${PKG_NAME}_main.js"
# sed "s/require[\(]'\.\/${PKG_NAME}/require\('\.\/${PKG_NAME}_main/" "pkg-node/${PKG_NAME}_bg.js" > "pkg/${PKG_NAME}_bg.js"
jq ".files += [\"${PKG_NAME}_bg.js\"]" pkg/package.json \
    | jq ".main = \"${PKG_NAME}_main.js\"" > pkg/temp.json
mv pkg/temp.json pkg/package.json
rm -rf pkg-node

