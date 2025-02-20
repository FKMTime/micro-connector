import * as wasm from "./pkg/hil_processor_wasm.js";

let state = wasm.init();
let res = state.feed_packet('{"tag":null,"type":"RequestToConnectDevice","data":{"espId":123456789,"type":"STATION"}}');
console.log(res);

let res2 = state.generate_output();
console.log(res2.split('\0'));

state.test((dsa) => {
    console.log(dsa);
});
