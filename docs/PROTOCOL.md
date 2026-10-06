# MS43 Diagnostic Protocol (DS2 over K-line)

Status: **research complete, implemented in `crates/ms43-core`, pending first in-car validation.**

This document records what the Siemens MS43 (BMW E46 330i, M54B30) actually speaks on the
OBD-II K-line, where every fact comes from, and what is still unverified. Nothing in the
code base uses a byte value that is not listed here with a source.

---

## 1. Summary

| Item | Value | Confidence |
|---|---|---|
| Protocol | **BMW DS2** (not KWP2000, not OBD-II PIDs) | Verified — BMW SGBD `MS430DS0` uses EDIABAS concept 6 = DS2 [S1][S4] |
| Physical | K-line, OBD-II pin 7 (pins 7+8 bridged on E46 — see §8) | Verified [S2] |
| Serial | **9600 baud, 8 data bits, even parity, 1 stop bit (8E1)** | Verified [S1][S3][S5][S6] |
| ECU (DME) address | **0x12** | Verified [S1][S3][S5][S6] |
| Wake-up / init | **None.** No 5-baud init, no fast init, no session start | Verified — SGBD `INITIALISIERUNG` only calls `xsetpar`/`xawlen` [S1] |
| Keep-alive / tester present | **None** | Verified — no `xsendf`/frequent telegram in SGBD init [S1] |
| Checksum | XOR of every byte of the frame except the checksum itself | Verified [S3][S4][S5], and recomputed on real captures (§6) |
| Echo | K-line is half-duplex single wire: a K+DCAN cable **echoes every transmitted byte** | Verified [S4] (EdiabasLib removes and checks remote echo) |
| Response timeout | 2000 ms (BMW `TimeoutStd`) | Verified [S1][S4] |
| Inter-byte / telegram end timeout | 20 ms (BMW `TimeoutTelEnd`) | Verified [S1][S4] |
| Regeneration time (gap after a response before next request) | 100 ms in BMW's SGBD | Verified [S1][S4] — but see §7: RomRaider/TunerPro poll without it |
| Retries | 2 repetitions on error (`xreps 2`) | Verified [S1] |

### A note on the conflicting "MS43 uses KWP2000" claim

One search-engine summary of the `staalebk/ds2` project claimed "MS43 uses KWP2000 instead of
DS2". **This conflicts with BMW's own diagnostic description file for MS43** (`MS430DS0`,
"MS 43.0 fuer M54 mit EWS 3", BMW TI-431, revision 2.00, 2004), whose `INITIALISIERUNG` job sets
EDIABAS concept `0x0006` = DS2 [S1][S4], and with RomRaider, TunerPro/MS4X and bmwe46oil, all of
which talk DS2 to MS43 successfully [S3][S5][S6]. We implement DS2. (KWP-style services appear
in MS43 flash/boot tooling, which this project deliberately does not touch.)

---

## 2. Frame format

```
 byte 0      byte 1        byte 2 .. n-2            byte n-1
+---------+-------------+-----------------------+----------+
| address | total length| payload               | checksum |
+---------+-------------+-----------------------+----------+
```

* `address` — 0x12 for the DME, in both directions.
* `total length` — the length of the **entire frame**, including address, length and checksum.
* `checksum` — XOR of bytes 0 .. n-2.
* Responses: payload byte 0 (frame byte 2) is the **status byte**. `0xA0` = OK.

Worked example (verified by hand and in unit tests):

```
12 05 0B 03 1F      0x12 ^ 0x05 ^ 0x0B ^ 0x03 = 0x1F
12 04 00 16         0x12 ^ 0x04 ^ 0x00 = 0x16
```

### Status byte (BMW `JOBRESULT` table from MS430DS0 [S1])

| SB | BMW text | Meaning in this app |
|---|---|---|
| `A0` | OKAY | positive response |
| `A1` | BUSY | negative (retry later) |
| `A2` | ERROR_ECU_REJECTED | negative |
| `B0` | ERROR_ECU_PARAMETER | negative |
| `B1` | ERROR_ECU_FUNCTION | negative |
| `B2` | ERROR_ECU_NUMBER | negative |
| `FF` | ERROR_ECU_NACK | negative |
| others | see `ds2::status_text` | negative |

---

## 3. Transaction sequence (what goes on the wire)

Derived from EdiabasLib `TransDs2` [S4] and RomRaider `DS2ResponseProcessor` [S3]:

1. Wait until at least *regen time* has passed since the last response.
2. Discard any stale input bytes (resynchronisation point).
3. Write the request frame (checksum appended).
4. Read back exactly `len(request)` echo bytes (EdiabasLib: echo timeout 100 ms) and compare
   them with what was sent. Mismatch → error + flush.
5. Read the 2-byte header (address, length) with the 2000 ms response timeout.
6. Read the remaining `length − 2` bytes, each chunk within the 20 ms telegram-end timeout.
7. Verify XOR checksum, address and status byte.

Our implementation adds: skipping garbage bytes before a `0x12` header (counted and logged),
a quiet-period input flush after any error, explicit cable-disconnect detection (any I/O error
that is not a timeout), and full raw TX / echo / RX tracing with timestamps.

**No-echo case.** If no echo arrives at all, the K-line transceiver in the cable is almost certainly
unpowered (cable not in the car, or OBD pin 16 has no 12 V). The app reports exactly that.

---

## 4. Commands used by this app (read-only allow-list)

All requests are built by `ms43_core::safety::Request`. Its constructors are the **only** way to
produce bytes for the wire and the transport re-checks every frame against the allow-list before
writing (§9).

| Request | Bytes (with checksum) | Response | Source |
|---|---|---|---|
| Identification (`IDENT`) | `12 04 00 16` | `12 LL A0 <ASCII ident> CS` | [S1] job `IDENT` sends `{12 04 00}`; [S3] `ECU_INIT_COMMAND` |
| Status block "Messwerte" (`STATUS_*`) | `12 05 0B 03 1F` | 45-byte frame, layout §5 | [S1] `BETRIEBSWTAB`; [S6] capture |
| Switch / flag block | `12 05 0B 04 18` | bit flags | [S3] (RomRaider group `0x0B 0x04`, groupsize 6) — layout community-sourced |
| VANOS actual/target block | `12 05 0B 90 8C` | layout §5.2 | [S1] `BETRIEBSWTAB` (`STATUS_VANOS_NW_LAGE_IST_SOLL_REF_*`) |
| Lambda adaptations | `12 05 0B 91 CS` | | [S1] |
| Throttle/idle adaptations | `12 05 0B 92 CS` | | [S1] |
| Knock-control octane adaptation | `12 05 0B 93 CS` | | [S1] |
| Read memory | `12 09 06 00 SG HI LO NN CS` | `12 LL A0 <NN bytes> CS` | [S3] `READ_MEMORY_COMMAND {06 00}`; MS41 example `12 09 06 00 00 DA 34 02` [S7] |

`SG HI LO` is the 24-bit C167 address (segment, high, low); `NN` = number of bytes. We limit
`NN` to 128, which is RomRaider's own range limit for this command [S3].

### Commands deliberately NOT implemented (blocked by the allow-list)

| Service | What it does | Why blocked |
|---|---|---|
| `07 ..` | write memory (RAM) | write — forbidden by project rules |
| `0B 01 ..` | load a custom "telegram" address list into the ECU (RomRaider fast mode, then read with `0B 00`) | stores a list in ECU RAM; see §7.2 — needs your explicit decision |
| `91 ..` | change DS2 baud rate (e.g. to 125 000) | changes ECU comms state until power cycle; see §7.1 |
| `43 ..` | ECU reset | forbidden |
| `05 ..`, `04 ..` | fault memory clear/read | out of scope (clear is a write) |
| `0C ..`, `22 ..`, `2B ..`, `40 ..` | actuator tests, system checks, adaptations | outside read-only scope |
| anything else | | default-deny |

---

## 5. Status block layouts

Offsets below are **frame byte indices** (0 = address byte; byte 3 = first data byte after `A0`).
Data type codes are the SGBD `DATA_TYPE` column; their meaning was taken from the SGBD's own
decoding routine [S1]:

| DATA_TYPE | Bytes | Byte order | Signedness | Evidence in SGBD code |
|---|---|---|---|---|
| 2 | 1 | – | unsigned | `B4=S2[i]; B5=0; fix2flt I2` |
| 3 | 1 | – | signed | `fix2flt B4` (separate path from type 2) |
| 4 | 2 | little-endian | unsigned | `B4=S2[i]; B5=S2[i+1]; clear I3; fix2flt L1` |
| 5 | 2 | **big-endian** | unsigned | `B5=S2[i]; B4=S2[i+1]; clear I3; fix2flt L1` |
| 6 | 2 | little-endian | signed | `fix2flt I2` |
| 7 | 2 | **big-endian** | signed | `fix2flt I2` |

All conversions in this table are linear: `physical = FACT_A × raw + FACT_B` (`COMPU_TYPE "--"`).

### 5.1 `0B 03` — main measurement block (45-byte response)

Source: MS430DS0 `BETRIEBSWTAB` rows with `TELEGRAM = 12050B03` [S1].
"Capture" = decoded value of the real E46 330i MS43 frame from bmwe46oil [S6] (warm idle):
`12 2D A0 02 C3 00 00 00 00 38 00 38 99 BB B5 A7 C8 01 88 FC 4E 64 35 9C 79 88 7C 2F 7C 55 FE FE FE FE 14 47 09 76 05 05 0D 2E 90 86 C2`

| Byte | Type | SGBD name | Job | Formula | Unit | Capture |
|---|---|---|---|---|---|---|
| 3 | 5 | N | STATUS_MOTORDREHZAHL | x | rpm | **707** |
| 5 | 2 | VS | STATUS_GESCHWINDIGKEIT | x | km/h | 0 |
| 6 | 5 | PVS_AV | STATUS_PEDALWERT | 0.0018311·x | ° pedal | 0.0 |
| 8 | 5 | TPS_AV_PRJ | STATUS_DK_WERT | 0.0018311·x | ° throttle | 0.10 |
| 10 | 5 | MAF_KGH | STATUS_LMM_MASSE | 0.25·x | kg/h | 14.0 |
| 12 | 2 | TIA | STATUS_AN_LUFTTEMPERATUR | 0.75·x − 48 | °C | 66.8 |
| 13 | 2 | TCO | STATUS_MOTORTEMPERATUR | 0.75·x − 48 | °C | 92.3 |
| 14 | 2 | TOIL | STATUS_OEL_TEMPERATUR | 0.796·x − 48 | °C | 96.1 ✔ INPA |
| 15 | 2 | TCO_EX | STATUS_KUEHLW_AUSL_TEMPERATUR | 0.75·x − 48 | °C | 77.3 |
| 16 | 2 | IGA_1 | STATUS_ZUENDWINKEL | −0.375·x + 72 | ° crank | −3.0 |
| 17 | 5 | TI_BANK_1 | STATUS_EINSPRITZZEIT | 0.0053333·x | ms | 2.09 |
| 19 | 7 | ISAPWM_IS | STATUS_LL_INTEGRATOR | 0.0015·x | % | −1.42 |
| 21 | 5 | ISAPWM_ISA | STATUS_LL_STELLER_TV | 0.0015·x | % | 38.5 |
| 23 | 2 | CAM_AV_IN | STATUS_VANOS_NW_LAGE_EINLASS | 0.375·x + 60 | ° crank | 118.5 |
| 24 | 3 | CAM_AV_EX | (exhaust cam actual) | −0.375·x − 60 | ° crank | −105.4 |
| 25 | 2 | VB_IGK | STATUS_KL15 | 0.10156·x | V | 13.8 |
| 26 | 5 | LAM_1 | STATUS_LAMBDA_INTEGRATOR_1 | 0.000015259·x + 0.5 | factor | 0.985 |
| 28 | 5 | LAM_2 | STATUS_LAMBDA_INTEGRATOR_2 | 0.000015259·x + 0.5 | factor | 0.986 |
| 30–33 | 2 | LSHPWM_* | O2 heater duty (up 1/2, down 1/2) | 0.391·x | % | 99.3 |
| 34 | 5 | MAF_MES | STATUS_LAST (load) | 0.0212·x | mg/stroke | 110.0 |
| 36 | 5 | NL_2 | STATUS_K_MW_2 (knock sensor 2 signal) | 0.0000778·x | V | 0.19 |
| 38 | 5 | NL_5 | STATUS_K_MW_5 (knock sensor 5 signal) | 0.0000778·x | V | 0.10 |
| 40 | 2 | ECFPWM_ECF | electric fan duty | 0.39063·x | % | 5.1 |
| 41 | 5 | AMP_MES | STATUS_LUFTDRUCK | 0.08292·x | hPa | 988.4 |
| 43 | 2 | VB | STATUS_UBATT | 0.10156·x | V | 13.6 |

Cross-checks performed:

* **Frame integrity** — length byte 0x2D = 45 = actual length; XOR checksum 0xC2 recomputes correctly.
* **Byte order** — RPM 0x02C3 = 707 and pressure 0x2E90 → 988 hPa are only plausible big-endian.
* **Physical plausibility** — every field above is consistent with a warm engine at idle
  (KL15 13.8 V, 14 kg/h MAF ↔ 110 mg/stroke at 707 rpm, coolant 92 °C, baro 988 hPa).
* **Independent calibration** — bmwe46oil fitted oil temperature against 19 INPA screen readings
  and got 0.796098·x − 48.0137 at this exact byte; the SGBD says 0.796·x − 48.
* **RomRaider agreement** — RomRaider MS43 definitions for `0B 03` use the same offsets measured
  from the first data byte (IPW at 0x0E → frame byte 17, VANOS in 0x14 → 23, battery 0x28 → 43)
  and the same scalings [S3-forum].
* **Logger.S agreement** — the `handmade0octopus/ds2` example reads battery voltage at data
  offset 22 (= frame byte 25, `VB_IGK`) with ×0.1 for MS43 part numbers 7511570, 7519308,
  7545150, 7551615 [S9].

**The `0B 03` layout is DME-family specific.** The same Logger.S table uses data offset **20** for
MS41/MS42 part numbers (1429764, 1430844, 7526753, 7500255), and MS41's `0B 03` is a different,
shorter block [S7]. The app therefore checks `ID_BMW_NR` against the known MS43 list before
trusting this table, and warns (but still shows raw frames) for unknown part numbers.

**Conflict noted:** exhaust cam (byte 24) is DATA_TYPE 3 (signed) in the `0B 03` table but
DATA_TYPE 2 (unsigned) in the `0B 90` table, and RomRaider treats it as unsigned. The values only
differ when raw > 127 (would be > −107.6 ° vs < −107.6 °). We follow the `0B 03` row (signed) and
flag it for in-car validation.

### 5.2 `0B 90` — VANOS actual / target block (SGBD only, no capture yet)

| Byte | Type | SGBD name | Formula | Unit |
|---|---|---|---|---|
| 3 | 2 | CAM_AV_IN (intake actual) | 0.375·x + 60 | ° crank |
| 4 | 2 | CAM_SP_IN (intake **target**) | 0.375·x + 60 | ° crank |
| 5 | 5 | CAM_REF_IN | 0.001465·x + 60 | ° crank |
| 7 | 3 | CAM_AD_IN (edge adaptation) | 0.375·x | ° crank |
| 8 | 2 | CAM_AV_IN_RAW | 0.375·x + 60 | ° crank |
| 9 | 2 | CAM_AV_EX (exhaust actual) | −0.375·x − 60 | ° crank |
| 10 | 2 | CAM_SP_EX (exhaust **target**) | −0.375·x − 60 | ° crank |
| 11 | 5 | CAM_REF_EX | −0.001465·x − 60 | ° crank |
| 13 | 3 | CAM_AD_EX | 0.375·x | ° crank |
| 14 | 2 | CAM_AV_EX_RAW | −0.375·x − 60 | ° crank |

This is the only verified source of **VANOS targets**. It costs a second request per cycle.

---

## 6. Identification (`IDENT`) response

The SGBD `IDENT` job slices ASCII substrings out of the full response telegram [S1]:

| Result | Frame offset | Length | Meaning |
|---|---|---|---|
| ID_BMW_NR | 3 | 7 | BMW part number (e.g. `7551615`) |
| ID_HW_NR | 10 | 2 | hardware number |
| ID_COD_INDEX | 12 | 2 | coding index |
| ID_DIAG_INDEX | 14 | 2 | diagnostic index |
| ID_BUS_INDEX | 16 | 2 | bus index |
| ID_DATUM_KW | 18 | 2 | production week |
| ID_DATUM_JAHR | 20 | 2 | production year |
| ID_LIEF_NR | 22 | 10 | supplier number (field as sliced by the SGBD; see TODO) |
| ID_SW_NR | 32 | 2 | software number |
| ID_AI_NR | 34 | 2 | change index |
| ID_PROD_NR | 37 | 8 | production number |

`ID_BMW_NR` is what RomRaider uses to choose a software-specific RAM address table
(`7551615` = MS43 software 430069; `7519308`, `7545150` = earlier releases [S3-defs]).

A real DS2 ident frame used as a framing fixture (from RomRaider source comment, a DS2 Siemens DME):
`12 2E A0 31 34 33 37 38 30 36 … 34 9C` — length and checksum verified.

TODO(validate): `ID_LIEF_NR` length (10) is unusual; confirm on your car's ident frame.

---

## 7. Speed: what limits the sample rate

At 8E1 each byte is 11 bits → **1.146 ms per byte at 9600 baud**.

| Exchange | Bytes on wire | Wire time |
|---|---|---|
| `0B 03` request + echo + 45-byte response | 5 + 45 | ≈ 57 ms |
| `0B 90` request + response (~16 bytes) | 5 + ~16 | ≈ 24 ms |
| 6-byte memory read | 9 + 10 | ≈ 22 ms |

So `0B 03` alone tops out around **15 frames/s** before ECU processing time; RomRaider reports
8–14 queries/s on DS2 ECUs [S7]. Every `0B 03` frame carries ~25 channels, so "samples/s" is far
higher than "requests/s".

### 7.1 Baud-rate switching (`0x91`) — NOT implemented, conflicting sources

The MS4X/RomRaider community reports MS42/43 accept a DS2 command to change baud (up to
125 000) only while the engine is not running, sticking until power-cycle [S7][S8]. The posted
frames **conflict**: one poster gave `12 08 91 00 25 80 00 2E` for 125 000, a second poster said
that pair was swapped (`…01 E8 48…` = 0x1E848 = 125 000, `…00 25 80…` = 0x2580 = 9600).
The same thread reports **no real gain** (14 queries/s at both 9600 and 125 000) because the ECU
paces its responses. Not implemented. TODO if ever needed: validate on the bench.

### 7.2 RomRaider "telegram" fast mode (`0B 01` + `0B 00`) — NOT enabled

RomRaider's MS43 fork loads a list of `[type, 4-byte address]` entries with
`12 LL 0B 01 N …` (ACK `12 04 A0 B6`), then repeatedly reads all of them with `12 05 0B 00 1C`
[S3]. This is how MS4X/RomRaider reach ~25 Hz with small packets, and it is the only community
route to per-cylinder ignition, knock retard and gear in one request.

It **stores the list in ECU RAM**. That is not a calibration or engine-variable write, but it is a
write-type service, so it is blocked until you decide. The addresses are also
software-version-specific (§10). Proposed policy: opt-in per run, only when `ID_BMW_NR` matches
a known table.

### 7.3 Regen time

BMW's SGBD waits 100 ms after each response before the next request (EdiabasLib honours it).
That alone would cap `0B 03` at ~6 Hz. RomRaider and TunerPro poll back-to-back and report stable
logging [S7][S8]. The app makes regen configurable (CLI `--regen-ms`, default **100 ms** for first
contact). Stage 6 of the test plan measures the effect of 0 ms.

---

## 8. Cable

* INPA-compatible **K+DCAN** USB cable (FTDI FT232R based). On an E46 (pre-2007) the cable's
  switch must bridge **OBD pins 7 and 8** (or the cable must be in "K-line" mode) [S2].
* EdiabasLib opens the port with DTR = RTS = off and relies on the cable's echo [S4]; we do the same
  (both configurable).
* MS4X recommends verifying the FTDI **latency timer** in Windows Device Manager; their page
  states 16 ms [S2]. The latency timer bounds how fast short reads come back; we will measure lower
  values in Stage 6 rather than assume.

---

## 9. Read-only safety design

* `safety::Request` has private fields; the only constructors are `ident()`,
  `status_block(StatusBlock)` and `read_memory(addr, len)`.
* `StatusBlock` is a closed enum of `0B xx` read blocks (`03 04 90 91 92 93`).
* `Transport::transact` re-validates the encoded payload with `safety::check_payload` immediately
  before writing — defence in depth against future code paths.
* `check_payload` is a **default-deny allow-list**, with unit tests that assert `07`, `0B 01`,
  `91`, `43`, `05`, `0C` and arbitrary bytes are rejected.
* Channel definitions reference a `StatusBlock` or a memory address — never raw bytes — so a
  channel definition cannot express a write.

---

## 10. Software-specific RAM addresses (community, unverified)

From the RomRaider MS43 logger definition maintained by ba114/Rustle333 with the MS4X team
[S3-defs]. Readable via `06 00` (pure read), keyed by `ID_BMW_NR`:

| Channel | 7551615 (430069) | 7519308 / 7545150 | Type | Formula |
|---|---|---|---|---|
| Knock retard "Knock1_6" (E27) | `0x00FB6E` | `0x00FB6E` | u8 | (x − 128)·0.375 ° |
| Ignition cyl 1 / 2 / 3 / 4 / 5 / 6 | `0x00FBAB/FBAF/FBAD/FBB0/FBAC/FBAE` | same | u8 | −0.375·x + 72 ° |
| Gear (`ov_gear`) | `0x0401AC` | `0x0401A7` | u8 | x |

TODO(validate): these were published for the telegram (`0B 01`) path. Reading them with `06 00`
is the same RAM, but byte-for-byte equivalence must be checked in-car before they are trusted.
Per-cylinder ignition is contiguous (`FBAB..FBB0`), so one 6-byte read covers all six.

---

## 11. Not available / unverified

| Wanted channel | Status |
|---|---|
| Commanded lambda / AFR | MS43 uses narrowband pre-cat sensors; no verified target-lambda channel found. TODO |
| Per-cylinder knock retard | Only a single "Knock1_6" byte is published; per-cylinder retard not found. TODO |
| Pre-cat O2 voltages | Community ADC "procedure" channels (`0B 02 0E 00 00 01/02`) [S7] — not enabled (unverified for MS43) |
| Fuel trims | `0B 91` lambda adaptations (SGBD) — block allowed, channels to be added after Stage 5 |
| DISA status | Community bit in `0B 04` byte 3 bit 2 — unverified |

---

## Sources

* **[S1]** BMW SGBD `MS430DS0` ("MS 43.0 fuer M54 mit EWS 3", BMW TI-431, rev 2.00, 2004),
  disassembled, in [radelbro/BimmerDis](https://github.com/radelbro/BimmerDis)
  (`assets/example/ms430ds0.b1v`, `ms430ds0_ds2.b1v`): jobs `INITIALISIERUNG`, `IDENT`,
  tables `BETRIEBSWTAB`, `JOBRESULT`.
* **[S2]** [MS4X Wiki — How to connect](https://www.ms4x.net/index.php?title=How_to_connect).
* **[S3]** [RomRaider source](https://github.com/RomRaider/RomRaider) — `io/protocol/ds2/iso9141/DS2Protocol.java`,
  `DS2ResponseProcessor.java`, `DS2ChecksumCalculator.java`, `logger/.../DS2LoggerConnection.java`.
  **[S3-defs]** [Rustle333/Siemens-MS43](https://github.com/Rustle333/Siemens-MS43) logger definitions.
  **[S3-forum]** RomRaider forum "DS2 – datalogging MS43 by serial protocol" (t=15316).
* **[S4]** [EdiabasLib](https://github.com/uholeschak/ediabaslib) — `EdInterfaceObd.cs`
  (concept 6 parameter mapping, `TransDs2`), `EdOperations.cs` (`xsetpar` word decoding).
* **[S5]** [staalebk/ds2](https://github.com/staalebk/ds2) — DS2 telegram structure.
* **[S6]** [tomicooler/bmwe46oil](https://github.com/tomicooler/bmwe46oil) — real MS43 (E46 330i)
  `0B 03` capture and INPA-calibrated oil temperature.
* **[S7]** RomRaider forum "DS2 – datalogging MS41/42/43 by serial protocol" (t=11086).
* **[S8]** [MS4X Wiki — TunerPro Data Logging](https://www.ms4x.net/index.php?title=TunerPro_Data_Logging).
* **[S9]** [handmade0octopus/ds2](https://github.com/handmade0octopus/ds2) — DS2 K-line library behind
  Logger.S (sorek.uk): 9600 8E1, echo handling, `12 04 00 16`, `12 05 0B 03 1F`, big-endian
  `getInt`, MS43 part-number → offset table. (`staalebk/ds2` is a fork of this repo.)
