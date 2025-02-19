import * as wasm from "./pkg/hil_processor_wasm.js";

let state = wasm.init();
let res = state.test('{"tag":null,"type":"RequestToConnectDevice","data":{"espId":123456789,"type":"STATION"}}');
console.log(res.split('\0'));
