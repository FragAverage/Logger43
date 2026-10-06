# MS43 channel status

Generated from `crates/ms43-core/src/channels.rs`. That file is the only place offsets and
scalings live. **No channel has been confirmed on your car yet.** That is Stage 4/5 of
[TESTING.md](TESTING.md).

## Verification levels

| Level | Meaning |
|---|---|
| **CrossChecked** | BMW MS430DS0 SGBD definition **and** decoded from a real MS43 capture to a physically plausible value **and** at least one independent tool agrees. |
| **Sgbd** | BMW MS430DS0 SGBD only. Offset and scaling are BMW's, but not yet seen in a real capture. |
| **Community** | RomRaider/MS4X reverse-engineering. DME-software specific RAM address, gated on `ID_BMW_NR`. |
| **Unavailable** | Requested but no verifiable source found. |

## Your requested channels

| Requested | Channel id | Source | Level | Notes |
|---|---|---|---|---|
| RPM | `rpm` | 0B03 @3 u16 BE | **CrossChecked** | First milestone channel |
| Vehicle speed | `vehicle_speed_kph` | 0B03 @5 u8 | Sgbd | Capture was stationary (0) |
| Throttle position | `throttle_deg` | 0B03 @8 u16 BE ×0.0018311 | CrossChecked | BMW unit is **degrees**, not % |
| Accelerator pedal | `pedal_deg` | 0B03 @6 u16 BE ×0.0018311 | Sgbd | degrees; capture was 0 |
| Engine load | `load_mg_stroke` | 0B03 @34 u16 BE ×0.0212 | CrossChecked | mg/stroke |
| MAF | `maf_kg_h`, `maf_g_s` | 0B03 @10 u16 BE ×0.25 | CrossChecked | g/s = kg/h ÷ 3.6 |
| Coolant temp | `coolant_c` | 0B03 @13 ×0.75−48 | CrossChecked | |
| Intake air temp | `iat_c` | 0B03 @12 ×0.75−48 | CrossChecked | |
| Battery voltage | `battery_v` | 0B03 @43 ×0.10156 | CrossChecked | also `kl15_v` @25 |
| Ignition angle | `ignition_deg` | 0B03 @16 −0.375x+72 | CrossChecked | cylinder 1 (`IGA_1`) |
| Knock retard | `knock_retard_deg` | RAM 0x00FB6E (x−128)·0.375 | Community | single value, see notes |
| Per-cylinder knock correction | — | — | **Unavailable** | not found in any verifiable source |
| Per-cylinder ignition | `ign_cyl_1..6` | RAM 0x00FBAB..FBB0 | Community | one 6-byte read |
| Injection time | `injection_ms` | 0B03 @17 u16 BE ×0.0053333 | CrossChecked | bank 1 |
| Commanded lambda / AFR | — | — | **Unavailable** | narrowband system; no target channel found |
| Lambda control | `lambda_int_1/2` | 0B03 @26/28 | CrossChecked | correction factor, not measured lambda |
| Pre-cat O2 voltage | — | ADC procedure `0B 02` | **Unavailable** (not enabled) | community-only for MS43 |
| VANOS intake actual | `vanos_intake_actual` | 0B03 @23 ×0.375+60 | CrossChecked | |
| VANOS exhaust actual | `vanos_exhaust_actual` | 0B03 @24 s8 −0.375x−60 | CrossChecked | signedness conflict, see PROTOCOL §5.1 |
| VANOS intake target | `vanos_intake_target` | 0B90 @4 ×0.375+60 | Sgbd | costs a 2nd request |
| VANOS exhaust target | `vanos_exhaust_target` | 0B90 @10 −0.375x−60 | Sgbd | costs a 2nd request |
| Gear | `gear` | RAM 0x0401AC / 0x0401A7 | Community | software specific |
| Fuel trims | (`0B 91` block) | SGBD | to add after Stage 5 | |
| Idle control | `idle_integrator_pct`, `idle_actuator_pct` | 0B03 @19/21 | CrossChecked | |
| Knock sensor signals | `knock_signal_2_v`, `knock_signal_5_v` | 0B03 @36/38 | CrossChecked | raw sensor level, not retard |
| DISA status | — | `0B 04` bit (community) | not added | unverified |
| Torque request | — | — | **Unavailable** | |

Extras from the same `0B 03` frame (free, no extra request): oil temp (CrossChecked against
INPA), radiator outlet temp, KL15 voltage, fan duty, ambient pressure.
