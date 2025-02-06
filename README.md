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
- Run it using `cargo run`
- Connect devices to it and it will start updating!

Why is this required? Devices with firmware < `2.4` are 
communicating using different packet structures. 
