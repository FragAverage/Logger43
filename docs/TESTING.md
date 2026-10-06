# Staged test plan

Run each stage in order. Do not move on until it passes. Always pass `--log run-stageN.log` so
there is a raw record to diagnose. Everything here is read-only.

Commands assume Windows (`ms43.exe`, `COM4`). On macOS use `/dev/cu.usbserial-XXXX`.

## Stage 0 — no hardware (sanity check)

```
ms43 poc --simulate --count 10
```
Expect: SIMULATION banner, an ident with part number `SIM4300` (with a not-on-list warning),
10 RPM values sweeping, about 70 ms RTT, 0 failures.

## Stage 1 — cable connected to PC only

```
ms43 ports
```
Expect: an entry marked `FTDI, likely K+DCAN` (USB 0403:6001). Note the port name.
Then `ms43 ident --port COM4`. Expect (on most cables): **"no echo from K-line"**. The K-line
transceiver in a K+DCAN cable is normally powered from OBD pin 16, so with no car there is
usually no echo. Either way, the port opened at 9600 8E1 without error, which is what this stage checks.

## Stage 2 — in car, ignition ON, engine OFF

Cable switch on the pin 7+8 (K-line) position.
```
ms43 ident --port COM4 --log stage2.log
```
Pass: a TX/EC/RX trio, `status OKAY`, a 7-digit BMW part number.
**Please send me the RX line and the part number.** It confirms the IDENT layout and,
if the part number is not 7551615/7545150/7519308/7511570, it goes on the known list.

## Stage 3 — repeated identification

```
ms43 ident --port COM4 --repeat 20 --log stage3.log
```
Pass: 20/20 identical idents, no `!!` lines, attempts = 1.

## Stage 4 — RPM at idle

Start the engine, let it idle.
```
ms43 poc --port COM4 --count 50 --log stage4.log
```
Pass: RPM within ±50 of the tachometer and stable at roughly 650–800 warm. Rev gently: RPM follows.
Expect about 5–6 requests/s at the default 100 ms regen.

## Stage 5 — more channels

```
ms43 poll --port COM4 --count 50 --all --log stage5.log
```
Compare against reality: coolant vs dash/INPA, battery ~13.5–14.5 V running,
baro ~950–1030 hPa, MAF ~12–18 kg/h warm idle, VANOS intake changes with revs.
Then `ms43 poll --port COM4 --block vanos --count 20` (first time `0B 90` is used: Sgbd-level channels).

## Stage 6 — latency and sample rate

```
ms43 poll --port COM4 --count 200 --quiet --regen-ms 100
ms43 poll --port COM4 --count 200 --quiet --regen-ms 20
ms43 poll --port COM4 --count 200 --quiet --regen-ms 0
```
Record req/s, min/avg/max RTT and failures for each. Optionally repeat with the FTDI latency timer
at 16 ms vs 2 ms (Device Manager → Port → Advanced). Use the fastest setting with **zero**
failures for logging.

## Stage 7 — Power Run logging

Engine idling, either headless:
```
ms43 log --port COM4 --preset power_run --seconds 60 --name idle-test --regen-ms <best from Stage 6>
```
or in the GUI (⚙ → regen, Power Run, Start log). Check run.csv opens in Excel, rows/s matches
Stage 6, `failed` and `dropped` ≈ 0, and run.json lists the ECU ident. If the part number is a
known MS43, check that `knock_retard_deg` is present and reads 0 at idle (first in-car check of a
Community RAM channel).

## Stage 8 — short road test

Passenger operates the laptop. One 3rd-gear pull. Check: no gaps, RPM curve smooth, failures ≈ 0.
