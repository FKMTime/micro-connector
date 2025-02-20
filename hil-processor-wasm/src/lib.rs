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
    pub fn feed_packet(&mut self, packet_str: &str) {
        let res = self.inner.feed(serde_json::from_str(packet_str).ok());
        if let Err(e) = res {
            unsafe { hil_log("ERROR", format!("Hil feed error! {e:?}")) };
        }
    }

    pub fn generate_output(&mut self) -> String {
        let mut out = String::new();
        if let Ok(packets) = self.inner.process() {
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

    pub fn test(&self, dsa: js_sys::Function) {
        dsa.call1(&JsValue::null(), &JsValue::from_f64(12.345));
    }
}

static TESTS_JSON: &str = include_str!("../../tests.json");

#[wasm_bindgen]
// TODO: add server state here?
pub fn init() -> WasmState {
    unsafe {
        hil_log(
            "INFO",
            format!("HilProcessor init! Version: {}", env!("CARGO_PKG_VERSION")),
        );
    };

    let state = HilState {
        tests: serde_json::from_str(TESTS_JSON).unwrap(),
        devices: Vec::new(),
        completed_count: 0,
        should_send_status: true,

        get_ms: || js_sys::Date::now() as u64,
        packet_queue: Vec::new(),
    };

    WasmState { inner: state }
}
