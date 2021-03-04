#!/usr/bin/env bash
set -euo pipefail

DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" >/dev/null 2>&1 && pwd )"
cd "$DIR"

# Clean previous packages
rm -rf lib lib.browser pkg pkg-node core

# Get the package name
PKG_NAME=${PWD##*/}

cargo build --lib --release --target wasm32-unknown-unknown

echo "running wasm-bindgen"
wasm-bindgen ../../target/wasm32-unknown-unknown/release/${PKG_NAME}.wasm --out-dir pkg --typescript --target bundler
wasm-bindgen ../../target/wasm32-unknown-unknown/release/${PKG_NAME}.wasm --out-dir pkg-node --typescript --target nodejs

if [ "1" != "${DISABLE_WASM_OPT-0}" ]; then
  echo "running wasm-opt"
  wasm-opt pkg/engine_bg.wasm -o pkg/engine_bg.wasm-opt.wasm -O3 --enable-mutable-globals
  wasm-opt pkg-node/engine_bg.wasm -o pkg-node/engine_bg.wasm-opt.wasm -O3 --enable-mutable-globals
  mv pkg/engine_bg.{wasm-opt.,}wasm
  mv pkg-node/engine_bg.{wasm-opt.,}wasm
else
  echo "skipping wasm-opt"
fi

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

yarn format