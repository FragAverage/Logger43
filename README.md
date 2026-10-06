# Logger43 — BMW MS43 data logger

Read-only live-data logger for the Siemens **MS43** DME (BMW E46 330i, M54B30) over a normal
**K+DCAN USB cable**, using BMW's **DS2** protocol on the K-line. Not an ELM327/OBD-II PID reader.

**Current state: built and tested against the simulator. Not yet validated in a car.**

* `ms43` CLI: proof of concept (ports → connect → ident → RPM with raw frames and latency) and headless logging.
* Tauri desktop app: live readouts and charts, presets (Power Run, Ignition/Knock, VANOS, Fueling, Idle, Raw Debug),
  priority scheduler, CSV + `run.json` logging, raw protocol debugger.

Validate in this order: CLI stages 1–6 of [docs/TESTING.md](docs/TESTING.md), then the GUI.

* Protocol research and sources: [docs/PROTOCOL.md](docs/PROTOCOL.md)
* Confirmed / unconfirmed channels: [docs/CHANNELS.md](docs/CHANNELS.md)
* Staged test plan: [docs/TESTING.md](docs/TESTING.md)

## Layout

```
crates/ms43-core/     protocol library (no Tauri dependency, unit-tested)
  src/serial.rs         port discovery, 9600 8E1 serial link
  src/transport.rs      echo, timeouts, resync, disconnect detection, raw trace
  src/ds2.rs            DS2 framing, XOR checksum, BMW status codes
  src/safety.rs         read-only request allow-list (the only way to build wire bytes)
  src/ms43.rs           IDENT parsing, known MS43 part numbers
  src/channels.rs       central channel definitions + verification levels
  src/presets.rs        Power Run and other presets
  src/plan.rs           selection -> DS2 requests, FAST/NORMAL/SLOW scheduling, RAM range merging
  src/engine.rs         poller thread, connection state machine, ring buffer, batched events
  src/logger.rs         CSV writer thread + run.json sidecar
  src/stats.rs          latency / rate statistics
  src/sim.rs            simulator (replays a real MS43 capture) for development
crates/ms43-cli/      `ms43` binary: PoC + headless logging
src-tauri/            Tauri shell: commands.rs (commands) + lib.rs (event forwarding)
src/                  React + TypeScript frontend
  components/           TopBar, ChannelPanel, Readouts, LogPanel
  pages/                LivePage, DebugPage
  charts/               LiveChart (uPlot)
  stores/               appStore (zustand), sampleStore (non-reactive sample buffer)
  types/ lib/           shared types (mirror of Rust serde), API wrappers, browser dev mock
docs/                 protocol, channels, test plan
```

This differs from the suggested `src-tauri/src/{serial,kwp,ms43,poller,logger,...}.rs` layout. The
protocol, poller and logger live in their own crate so the CLI and tests don't need Tauri,
and the GUI cannot bypass the safety layer. The module is `ds2.rs`, not `kwp.rs`, because MS43
speaks DS2, not KWP2000 (PROTOCOL.md §1). The poller is `engine.rs` + `plan.rs`.

### Data flow

```
K-line ─ transport ─ poller thread (engine.rs) ─ decode ─┬─► ring buffer (20 000 rows)
                                                          ├─► bounded channel ─► CSV writer thread ─► run.csv / run.json
                                                          └─► pending rows ─┐
                                    trace ─► debug ring (5000) ─────────────┤
                                                       emitter thread, every 100 ms ─► Tauri events ─► React
```

The poller never waits for the UI or the disk. A full logger queue or UI buffer increments a
"dropped" counter instead of slowing polling.

### Scheduler

Each selected channel names its source (a DS2 status block or a RAM address). The planner issues
one request per block per cycle (a `0B 03` frame carries ~25 values) and merges nearby RAM
addresses into single `06 00` reads. A request inherits the highest priority of its channels:
FAST = every cycle, NORMAL = every 3rd, SLOW = every 10th. Power Run is 0B03 + 0B90 (+ one RAM
read for knock retard on known DME software).

### Tauri commands and events

Commands: `list_serial_ports`, `connect_ecu`, `disconnect_ecu`, `get_status`, `get_ecu_info`,
`get_channels`, `get_presets`, `set_selected_channels`, `get_selected_channels`, `start_logging`,
`stop_logging`, `get_logger_stats`, `get_recent_samples`, `set_debug`, `start_raw_recording`,
`stop_raw_recording`, `save_debug_history`, `get_default_log_dir`.

Events: `connection_status`, `ecu_info`, `sample_batch` (columnar, ~10/s), `logger_stats` (2/s),
`protocol_debug` (only while capture is on), `protocol_error`. Types: `src/types/ms43.ts`.

### CSV / run.json

`Documents/Logger43/runs/<date>_<run name>/run.csv` has `timestamp_ms` (ms since log start,
monotonic clock) followed by one column per channel id (e.g. `rpm, load_mg_stroke, throttle_deg,
maf_g_s, ignition_deg, vanos_intake_target, …`). There is one row per poller cycle. Slower requests
not due in a cycle carry their last value forward; a due request that failed leaves its cells empty.
`run.json` records vehicle, ECU ident, port, notes, preset, every channel with its source, formula
and verification level, row count, average sample rate, polling statistics and connection settings.

Columns are named after what the ECU reports, so a few differ from a generic logger.
`throttle_deg` (BMW reports degrees, not %), `load_mg_stroke`, `lambda_int_1` (closed-loop
correction, not measured lambda). Per-cylinder knock and target lambda are not available
(see CHANNELS.md).

## Prerequisites

| Tool | Version | Install |
|---|---|---|
| Rust | stable (1.80+) | https://rustup.rs — on Windows run `rustup-init.exe`, accept the defaults (MSVC toolchain) |
| Node.js | 20 LTS or newer | https://nodejs.org (Windows installer), or `winget install OpenJS.NodeJS.LTS` |
| Windows build tools | VS 2022 Build Tools, "Desktop development with C++" | https://visualstudio.microsoft.com/visual-cpp-build-tools/ |
| WebView2 | preinstalled on Windows 10/11 | otherwise https://developer.microsoft.com/microsoft-edge/webview2/ |
| macOS | Xcode Command Line Tools | `xcode-select --install` |

Full Tauri prerequisites: https://v2.tauri.app/start/prerequisites/

## Build and run (Windows)

```powershell
git clone <this repo> Logger43
cd Logger43

# CLI proof of concept
cargo build --release -p ms43-cli
.\target\release\ms43.exe ports
.\target\release\ms43.exe poc --simulate          # no hardware needed
.\target\release\ms43.exe poc --port COM4         # real car

# tests
cargo test --workspace

# GUI
npm install
npm run tauri dev                                  # development mode
npm run tauri build                                # installer -> target\release\bundle\nsis\ and \msi\

# GUI against the simulator (no cable): pick "Simulator" in the port list, or
$env:LOGGER43_SIMULATE=1; npm run tauri dev       # auto-connects with Power Run

# UI-only work in a normal browser (synthetic MOCK backend, never bundled in builds)
npm run dev                                        # http://localhost:1420
```

## CLI reference

```
ms43 ports                                list serial ports, FTDI marked as likely K+DCAN
ms43 ident  --port COM4 [--repeat 20]     read DME identification
ms43 poll   --port COM4 [--count N] [--all] [--block measurements|vanos]
ms43 poc    --port COM4 [--count N]       full milestone-1 sequence
ms43 log    --port COM4 --preset power_run --seconds 30 --name "3rd gear" [--dir runs]
                                          headless CSV + run.json logging (same engine as the GUI)

common options:
  --simulate             simulator instead of a cable
  --log FILE             raw session log (ISO timestamps; TX/EC=echo/RX/XX=discarded/ER=error)
  --quiet                hide raw frames on the console
  --regen-ms 100         idle after each response (BMW: 100; try 0–20 in Stage 6)
  --timeout-ms 2000      response timeout (BMW: 2000)
  --inter-byte-ms 20     telegram gap timeout (BMW: 20)
  --usb-latency-ms 16    USB buffering allowance (match your FTDI latency timer)
  --no-echo              for interfaces that suppress the K-line echo
  --dtr / --rts          assert DTR/RTS (default off, as EdiabasLib)
```

Example output (simulator):

```
TX 19:53:15.174  [12 05 0B 03 1F]  (0B03 measurements)
EC 19:53:15.175  [12 05 0B 03 1F]
RX 19:53:15.236  [12 2D A0 04 03 00 00 ... 90 86 04]  (RTT 69.6 ms, status OKAY)
RTT: 69.6 ms  (ECU+USB delay after echo 11.7 ms)
Decoded: Engine speed = 1027.000 rpm  raw=0x0403 (1027) [CrossChecked]
```

## Finding the K+DCAN COM port (Windows)

1. Plug in the cable. Open **Device Manager → Ports (COM & LPT)**. Look for *USB Serial Port (COMx)*.
2. Or run `ms43 ports`: the FTDI entry (USB `0403:6001`) is marked `likely K+DCAN`.
3. If the port doesn't appear, install the FTDI VCP driver (https://ftdichip.com/drivers/vcp-drivers/).

## Recommended cable settings

* **Switch / pins:** E46 needs OBD **pins 7 and 8 bridged** (K-line mode). On switchable cables use the
  position for pre-2007 / K-line. ([MS4X: How to connect](https://www.ms4x.net/index.php?title=How_to_connect))
* **FTDI latency timer:** Device Manager → USB Serial Port → Properties → Port Settings →
  Advanced → *Latency Timer*. MS4X recommends verifying it is **16 ms**. Lower values may raise the
  sample rate; that is measured in Stage 6, not assumed. Pass the same value as `--usb-latency-ms`.
* Baud/parity are set by the app (9600, 8E1). The Device Manager values don't matter.
* Close INPA/TunerPro/RomRaider. Only one program can own the COM port.

## First communication test (ignition ON, engine OFF)

```
ms43 ident --port COM4 --log first-contact.log
```
You should see a TX line, an identical EC (echo) line, then an RX line with `status OKAY` and a
7-digit BMW part number. Then follow [docs/TESTING.md](docs/TESTING.md).

## Raw debug logs

**GUI:** *Raw debug* tab → *Live capture* streams every TX / echo / RX / discarded frame, with RTT,
checksum validity, decoded values with raw hex, the request plan and errors. *Record raw session
to file* writes everything to `Documents/Logger43/debug/raw_<time>.log`. *Save last 5000* dumps
the backend's history, which is kept even while capture is off, so you can save it after a problem.

**CLI:** add `--log session.log` to any command. Every byte sent, echoed, received or discarded is written
with a millisecond timestamp, plus every error. The console shows the same unless `--quiet`.
Send me these logs when a value looks wrong.

## Common connection problems

| Symptom | Likely cause / fix |
|---|---|
| `cannot open COM4: Access is denied` | another program (INPA, TunerPro, a previous `ms43`) holds the port |
| `no echo from K-line` | cable not powered (not in car / ignition off / OBD pin 16 fuse), or wrong COM port |
| `echo mismatch` | wrong baud/parity on a non-FTDI adapter, noisy line, or another tester on the bus |
| `timeout waiting for Header` after a good echo | pins 7/8 not bridged (switch position), ignition off, DME not answering on 0x12 |
| repeated `checksum mismatch` | bad cable/USB hub, latency timer too low, or interference |
| `serial link lost` | USB cable unplugged or the driver reset |
| part-number WARNING | DME not on the known MS43 list. Decoding continues; please report the number |
| RPM nonsense | stop and send the log. Never trust a channel the log doesn't support |

## Safety

Read-only by construction. The only requests that can be built are IDENT (`00`), status blocks
(`0B 03/04/90/91/92/93`) and memory reads (`06 00`, ≤ 128 bytes). Writes (`07`), telegram setup
(`0B 01`), baud change (`91`), reset (`43`), fault clearing, actuator tests and anything unknown
are rejected twice: at construction and again by the transport right before the bytes are
written. See PROTOCOL.md §9 and the tests in `safety.rs`.

Use on public roads at your own risk. Have a passenger operate the laptop.
