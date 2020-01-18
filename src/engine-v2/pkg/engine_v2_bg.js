
const path = require('path').join(__dirname, 'engine_v2_bg.wasm');
const bytes = require('fs').readFileSync(path);
let imports = {};
imports['./engine_v2.js'] = require('./engine_v2_main.js');

const wasmModule = new WebAssembly.Module(bytes);
const wasmInstance = new WebAssembly.Instance(wasmModule, imports);
module.exports = wasmInstance.exports;
