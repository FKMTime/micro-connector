#!/bin/bash
RUSTFLAGS='--cfg getrandom_backend="wasm_js"' wasm-pack build --target nodejs
#cp src/bindings.js ./pkg
node test.js

