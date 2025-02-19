use hil_processor::HilState;
use wasm_bindgen::prelude::*;

#[no_mangle]
pub unsafe extern "Rust" fn hil_log(tag: &str, content: String) {
    log(&format!("[{tag}] {content}"));
}

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = console)]
    fn log(s: &str);
}

#[wasm_bindgen]
pub struct WasmState {
    inner: HilState,
}

#[wasm_bindgen]
impl WasmState {
    pub fn test(&mut self, packet_str: &str) -> String {
        let res = self
            .inner
            .process_packet(serde_json::from_str(packet_str).ok());

        let mut out = String::new();
        if let Ok(packets) = res {
            for packet in packets {
                let res = serde_json::to_string(&packet);
                match res {
                    Ok(packet_str) => {
                        out.push_str(&packet_str);
                        out.push('\0'); // split packets using null byte
                    }
                    Err(e) => unsafe {
                        hil_log("ERROR", format!("{e:?}"));
                    },
                }
            }
        }

        out
    }
}

static TESTS_JSON: &str = include_str!("../../tests.json");
#[wasm_bindgen]
pub fn init() -> WasmState {
    log("Wasm init!");

    let state = HilState {
        tests: serde_json::from_str(TESTS_JSON).unwrap(),
        devices: Vec::new(),
        completed_count: 0,
        should_send_status: true,

        get_ms: || js_sys::Date::now() as u64,
    };

    WasmState { inner: state }
}
