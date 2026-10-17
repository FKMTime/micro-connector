# FKM micro-connector
Micro-connector connects FKM devices to FKMTime backend.

## Logging
To see logs for that backend only use:
```
RUST_LOG=none,backend=trace cargo run
```

## V2 hardware(V2 firmware) -> V3 firmware update
- Clone micro-connector's `olf-fw-update` branch
  ```bash
  git clone -b old-fw-update https://github.com/FKMTime/micro-connector
  ```
- Cd into cloned directory and run micro-connector using `FIRMWARE_DIR=/tmp/fkm-build cargo run`
- Connect devices to it and they will start updating!

Why is this required? Devices with firmware < `2.4` are 
communicating using different packet structures. 

## Fake FKMTime for device simulations

Build with `cargo build --bins`, then run `target/debug/e2e` from a directory containing
`tests.json`. This file defines `groups`, `cards`, and optional `rounds` (attempt counts,
cutoffs and time limits in centiseconds). The bundled configuration has sample cards and groups.

The fake server always models FKMTime: attempts are tracked by session, delegate calls become
incidents, and resolved incidents, extra attempts, cutoffs and limits follow the server flows.
`SOCKET_PATH` selects its connector Unix socket. `E2E_CONTROL` selects the delegate control
socket (default: `$SOCKET_PATH.ctl`); `E2E_LOG` selects the JSONL request log (default: `e2e.jsonl`).
The `e2e` executable name is retained for firmware sim scripts; `E2E_SIM` is no longer needed.
Drive device inputs through the firmware simulator's bus models.
