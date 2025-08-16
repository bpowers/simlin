# Makefile for engine2

.PHONY: all build test clean header wasm

all: build header

build:
	cargo build --release

wasm:
	./build.sh

test:
	cargo test -p engine2
	go test ./...

header:
	@echo "C header is manually maintained in simlin.h"

clean:
	cargo clean
	rm -f engine2.wasm