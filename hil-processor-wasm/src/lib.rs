use hil_processor::HilState;
use unix_utils::response::CompetitionStatusResp;
use wasm_bindgen::prelude::*;

static mut LOG_FUNC: Option<js_sys::Function> = None;

#[wasm_bindgen]
pub struct WasmState {
    inner: HilState,
    log_fn: fn(&str, String),
}

#[wasm_bindgen]
impl WasmState {
    pub fn feed_packet(&mut self, packet_str: &str) {
        let res = self.inner.feed(serde_json::from_str(packet_str).ok());
        if let Err(e) = res {
            (self.log_fn)("ERROR", format!("Hil feed error! {e:?}"));
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
                    Err(e) => {
                        (self.log_fn)("ERROR", format!("Output packet to string: {e:?}"));
                    }
                }
            }
        }

        out
    }

    pub fn set_status(&mut self, status: String) {
        if let Ok(status) = serde_json::from_str::<CompetitionStatusResp>(&status) {
            self.inner.status.should_update = status.should_update;
            self.inner.status.translations = status.translations;
            self.inner.status.default_locale = status.default_locale;
            self.inner.send_status_resp();
        }
    }
}

static TESTS_JSON: &str = include_str!("../../tests.json");

#[wasm_bindgen]
pub fn init(log_func: js_sys::Function, initial_status: Option<String>) -> WasmState {
    let initial_status: Option<CompetitionStatusResp> =
        initial_status.and_then(|status| serde_json::from_str(&status).ok());

    unsafe {
        LOG_FUNC = Some(log_func);
    }

    let log_fn: fn(&str, String) = |tag: &str, msg: String| unsafe {
        if let Some(ref l) = LOG_FUNC {
            _ = l.call2(
                &JsValue::null(),
                &JsValue::from_str(tag),
                &JsValue::from_str(&msg),
            );
        }
    };

    log_fn(
        "INFO",
        format!("HilProcessor init! Version: {}", env!("CARGO_PKG_VERSION")),
    );

    let mut state = HilState {
        tests: serde_json::from_str(TESTS_JSON).unwrap(),
        devices: Vec::new(),
        completed_count: 0,
        should_send_status: true,
        status: initial_status.unwrap_or_default(),

        get_ms: || js_sys::Date::now() as u64,
        packet_queue: Vec::new(),
        log_fn: log_fn.clone(),

        error_log: Vec::new(),
    };
    state.process_initial_status_devices();

    WasmState {
        inner: state,
        log_fn,
    }
}
