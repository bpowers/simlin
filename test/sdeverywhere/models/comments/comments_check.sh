#!/bin/bash

MODEL_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"
SDE='node ../../src/sde.js'

cd $MODEL_DIR
$SDE generate --preprocess comments.mdl

diff expected.mdl build/comments.mdl > build/diff.txt 2>&1
if [ $? != 0 ]; then
  echo
  echo "ERROR: 'sde generate --preprocess' produced unexpected results:"
  echo
  cat build/diff.txt
  echo
  exit 1
fi

echo "All validation checks passed!"
