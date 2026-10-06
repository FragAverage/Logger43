//! Central MS43 channel definitions.
//!
//! MS43 does not serve one value per request. A DS2 status block (`0B 03`, `0B 90`, …) returns a
//! fixed telegram with many values, and RAM values can be read with `06 00`. So a channel is
//! "where in which response" plus a linear conversion, not "request + PID". The poller groups the
//! selected channels by source and issues one request per block per cycle.
//!
//! Every definition carries its verification level and sources. The UI and CSV export must show
//! them, and nothing outside this module may hard-code offsets or scalings.

use serde::Serialize;

use crate::ds2::Response;
use crate::safety::StatusBlock;

/// Raw encodings, named after the MS430DS0 `DATA_TYPE` codes they correspond to (docs §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum DataType {
    /// SGBD type 2
    U8,
    /// SGBD type 3
    S8,
    /// SGBD type 4
    U16Le,
    /// SGBD type 5
    U16Be,
    /// SGBD type 6
    S16Le,
    /// SGBD type 7
    S16Be,
}

impl DataType {
    pub fn size(self) -> usize {
        match self {
            DataType::U8 | DataType::S8 => 1,
            _ => 2,
        }
    }

    /// Reads a raw value at `offset` of `bytes`. `None` if out of range.
    pub fn read(self, bytes: &[u8], offset: usize) -> Option<i64> {
        let s = bytes.get(offset..offset + self.size())?;
        Some(match self {
            DataType::U8 => s[0] as i64,
            DataType::S8 => s[0] as i8 as i64,
            DataType::U16Le => u16::from_le_bytes([s[0], s[1]]) as i64,
            DataType::U16Be => u16::from_be_bytes([s[0], s[1]]) as i64,
            DataType::S16Le => i16::from_le_bytes([s[0], s[1]]) as i64,
            DataType::S16Be => i16::from_be_bytes([s[0], s[1]]) as i64,
        })
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
pub enum Source {
    /// Value inside a status block response. `offset` is the frame byte index (0 = address byte).
    Block {
        block: StatusBlock,
        offset: usize,
        dtype: DataType,
    },
    /// ECU RAM read with `06 00`. Addresses are DME-software specific, keyed by `ID_BMW_NR`.
    Memory {
        addresses: &'static [(&'static str, u32)],
        dtype: DataType,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum Priority {
    Fast,
    Normal,
    Slow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Verification {
    /// BMW SGBD definition + decoded from a real MS43 capture to a physically plausible value
    /// + at least one independent tool agreeing. Still pending test on *your* car.
    CrossChecked,
    /// Offset and scaling from BMW's MS430DS0 SGBD only. Not yet seen in a real capture.
    Sgbd,
    /// Community reverse-engineering (RomRaider/MS4X), software-version specific, unverified.
    Community,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct ChannelDef {
    pub id: &'static str,
    pub label: &'static str,
    pub unit: &'static str,
    /// CSV column name.
    pub column: &'static str,
    pub source: Source,
    /// physical = factor * raw + offset (all MS430DS0 conversions are linear).
    pub factor: f64,
    pub offset: f64,
    pub priority: Priority,
    pub verification: Verification,
    pub sources: &'static str,
    pub note: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Decoded {
    pub raw: i64,
    pub value: f64,
}

impl ChannelDef {
    /// Decodes this channel from a status-block response. `None` for memory channels or if the
    /// response is too short for this offset.
    pub fn decode_block(&self, resp: &Response) -> Option<Decoded> {
        match self.source {
            Source::Block { offset, dtype, .. } => {
                let raw = dtype.read(resp.bytes(), offset)?;
                // never decode the checksum byte as data
                (offset + dtype.size() < resp.bytes().len()).then(|| self.convert(raw))
            }
            Source::Memory { .. } => None,
        }
    }

    pub fn convert(&self, raw: i64) -> Decoded {
        Decoded {
            raw,
            value: self.factor * raw as f64 + self.offset,
        }
    }

    pub fn block(&self) -> Option<StatusBlock> {
        match self.source {
            Source::Block { block, .. } => Some(block),
            Source::Memory { .. } => None,
        }
    }

    /// RAM address for a given DME part number, if this is a memory channel with a known address.
    pub fn memory_address(&self, part_number: &str) -> Option<u32> {
        match self.source {
            Source::Memory { addresses, .. } => addresses
                .iter()
                .find(|(p, _)| *p == part_number)
                .map(|(_, a)| *a),
            Source::Block { .. } => None,
        }
    }
}

/// UI/export view of a channel: definition plus derived group and human-readable source.
#[derive(Debug, Clone, Serialize)]
pub struct ChannelInfo {
    #[serde(flatten)]
    pub def: ChannelDef,
    pub group: &'static str,
    pub source_text: String,
}

impl ChannelDef {
    pub fn group(&self) -> &'static str {
        let id = self.id;
        if id.starts_with("vanos") {
            "VANOS"
        } else if id.starts_with("ign") || id.starts_with("knock") {
            "Ignition / knock"
        } else if id.starts_with("lambda") || id.starts_with("injection") || id.starts_with("maf") {
            "Fuel / air"
        } else if id.ends_with("_c") {
            "Temperatures"
        } else if id.ends_with("_v") || id == "fan_pct" || id == "baro_hpa" {
            "Electrical / ambient"
        } else if id.starts_with("idle") {
            "Idle"
        } else {
            "Engine"
        }
    }

    pub fn source_text(&self) -> String {
        match self.source {
            Source::Block {
                block,
                offset,
                dtype,
            } => format!("{} byte {offset} {dtype:?}", block.name()),
            Source::Memory { addresses, dtype } => {
                let a: Vec<String> = addresses
                    .iter()
                    .map(|(p, a)| format!("{p}:0x{a:06X}"))
                    .collect();
                format!("RAM 06 00 {dtype:?} [{}]", a.join(", "))
            }
        }
    }

    pub fn info(&self) -> ChannelInfo {
        ChannelInfo {
            def: *self,
            group: self.group(),
            source_text: self.source_text(),
        }
    }
}

pub fn find(id: &str) -> Option<&'static ChannelDef> {
    CHANNELS.iter().find(|c| c.id == id)
}

const SGBD: &str = "BMW MS430DS0 BETRIEBSWTAB";
const SGBD_CAP: &str = "BMW MS430DS0 BETRIEBSWTAB; real MS43 capture (bmwe46oil)";
const SGBD_CAP_RR: &str =
    "BMW MS430DS0 BETRIEBSWTAB; real MS43 capture (bmwe46oil); RomRaider MS43 defs";
const RR_DEFS: &str = "RomRaider MS43 logger defs (Rustle333/Siemens-MS43)";

const fn blk(block: StatusBlock, offset: usize, dtype: DataType) -> Source {
    Source::Block {
        block,
        offset,
        dtype,
    }
}
const M: StatusBlock = StatusBlock::Measurements;
const V: StatusBlock = StatusBlock::Vanos;

use DataType::*;
use Priority::*;
use Verification::*;

macro_rules! ch {
    ($id:literal, $label:literal, $unit:literal, $src:expr, $f:expr, $o:expr, $prio:expr, $ver:expr, $sources:expr) => {
        ch!($id, $label, $unit, $src, $f, $o, $prio, $ver, $sources, None)
    };
    ($id:literal, $label:literal, $unit:literal, $src:expr, $f:expr, $o:expr, $prio:expr, $ver:expr, $sources:expr, $note:expr) => {
        ChannelDef {
            id: $id,
            label: $label,
            unit: $unit,
            column: $id,
            source: $src,
            factor: $f,
            offset: $o,
            priority: $prio,
            verification: $ver,
            sources: $sources,
            note: $note,
        }
    };
}

/// Ignition per-cylinder and knock RAM addresses (RomRaider MS43 defs, telegram list).
const KNOCK_ADDR: &[(&str, u32)] = &[
    ("7551615", 0x00FB6E),
    ("7519308", 0x00FB6E),
    ("7545150", 0x00FB6E),
];
const fn ign(addr: u32) -> [(&'static str, u32); 3] {
    [("7551615", addr), ("7519308", addr), ("7545150", addr)]
}
const IGN1: [(&str, u32); 3] = ign(0x00FBAB);
const IGN2: [(&str, u32); 3] = ign(0x00FBAF);
const IGN3: [(&str, u32); 3] = ign(0x00FBAD);
const IGN4: [(&str, u32); 3] = ign(0x00FBB0);
const IGN5: [(&str, u32); 3] = ign(0x00FBAC);
const IGN6: [(&str, u32); 3] = ign(0x00FBAE);
const GEAR_ADDR: &[(&str, u32)] = &[
    ("7551615", 0x0401AC),
    ("7519308", 0x0401A7),
    ("7545150", 0x0401A7),
];

const RAM_NOTE: Option<&str> = Some(
    "Published for RomRaider's 0B01 telegram mode; read here with 06 00. Gated on ID_BMW_NR. Validate in car.",
);

pub static CHANNELS: &[ChannelDef] = &[
    // ---------------- 0B 03 measurement block (frame byte offsets) ----------------
    ch!("rpm", "Engine speed", "rpm", blk(M, 3, U16Be), 1.0, 0.0, Fast, CrossChecked, SGBD_CAP),
    ch!("vehicle_speed_kph", "Vehicle speed", "km/h", blk(M, 5, U8), 1.0, 0.0, Normal, Sgbd, SGBD,
        Some("Capture value was 0 (stationary): offset not discriminated by the capture.")),
    ch!("pedal_deg", "Accelerator pedal", "deg", blk(M, 6, U16Be), 0.0018311, 0.0, Normal, Sgbd, SGBD,
        Some("SGBD unit is degrees pedal (GradPWG), not percent.")),
    ch!("throttle_deg", "Throttle angle", "deg", blk(M, 8, U16Be), 0.0018311, 0.0, Normal, CrossChecked, SGBD_CAP,
        Some("SGBD unit is degrees throttle (GradDK), not percent.")),
    ch!("maf_kg_h", "Air mass", "kg/h", blk(M, 10, U16Be), 0.25, 0.0, Normal, CrossChecked, SGBD_CAP),
    ch!("maf_g_s", "Air mass", "g/s", blk(M, 10, U16Be), 0.25 / 3.6, 0.0, Normal, CrossChecked, SGBD_CAP,
        Some("Same raw value as maf_kg_h, converted kg/h -> g/s.")),
    ch!("iat_c", "Intake air temp", "°C", blk(M, 12, U8), 0.75, -48.0, Slow, CrossChecked, SGBD_CAP),
    ch!("coolant_c", "Coolant temp", "°C", blk(M, 13, U8), 0.75, -48.0, Slow, CrossChecked, SGBD_CAP),
    ch!("oil_c", "Oil temp", "°C", blk(M, 14, U8), 0.796, -48.0, Slow, CrossChecked,
        "BMW MS430DS0; bmwe46oil fit against 19 INPA readings (0.796098x-48.0137)"),
    ch!("radiator_out_c", "Radiator outlet temp", "°C", blk(M, 15, U8), 0.75, -48.0, Slow, CrossChecked, SGBD_CAP_RR),
    ch!("ignition_deg", "Ignition angle (cyl 1)", "° crank", blk(M, 16, U8), -0.375, 72.0, Fast, CrossChecked, SGBD_CAP,
        Some("SGBD name IGA_1 / STATUS_ZUENDWINKEL.")),
    ch!("injection_ms", "Injection time bank 1", "ms", blk(M, 17, U16Be), 0.0053333, 0.0, Normal, CrossChecked, SGBD_CAP_RR),
    ch!("idle_integrator_pct", "Idle control integrator", "%", blk(M, 19, S16Be), 0.0015, 0.0, Slow, CrossChecked, SGBD_CAP),
    ch!("idle_actuator_pct", "Idle actuator duty", "%", blk(M, 21, U16Be), 0.0015, 0.0, Slow, CrossChecked, SGBD_CAP_RR),
    ch!("vanos_intake_actual", "VANOS intake actual", "° crank", blk(M, 23, U8), 0.375, 60.0, Fast, CrossChecked, SGBD_CAP_RR),
    ch!("vanos_exhaust_actual", "VANOS exhaust actual", "° crank", blk(M, 24, S8), -0.375, -60.0, Fast, CrossChecked, SGBD_CAP_RR,
        Some("Signedness conflict: 0B03 row says signed, 0B90 row and RomRaider say unsigned. Differs only for raw > 127.")),
    ch!("kl15_v", "Ignition (KL15) voltage", "V", blk(M, 25, U8), 0.10156, 0.0, Slow, CrossChecked,
        "BMW MS430DS0; real MS43 capture; Logger.S battery offset 22 (x0.1)"),
    ch!("lambda_int_1", "Lambda integrator bank 1", "factor", blk(M, 26, U16Be), 0.000015259, 0.5, Normal, CrossChecked, SGBD_CAP,
        Some("Closed-loop correction factor, not measured lambda.")),
    ch!("lambda_int_2", "Lambda integrator bank 2", "factor", blk(M, 28, U16Be), 0.000015259, 0.5, Normal, CrossChecked, SGBD_CAP),
    ch!("load_mg_stroke", "Engine load", "mg/stroke", blk(M, 34, U16Be), 0.0212, 0.0, Fast, CrossChecked, SGBD_CAP,
        Some("STATUS_LAST. Capture: 110 mg/stroke at 707 rpm, consistent with 14 kg/h MAF.")),
    ch!("knock_signal_2_v", "Knock sensor signal 2", "V", blk(M, 36, U16Be), 0.0000778, 0.0, Normal, CrossChecked, SGBD_CAP_RR),
    ch!("knock_signal_5_v", "Knock sensor signal 5", "V", blk(M, 38, U16Be), 0.0000778, 0.0, Normal, CrossChecked, SGBD_CAP_RR),
    ch!("fan_pct", "Electric fan duty", "%", blk(M, 40, U8), 0.39063, 0.0, Slow, CrossChecked, SGBD_CAP_RR),
    ch!("baro_hpa", "Ambient pressure", "hPa", blk(M, 41, U16Be), 0.08292, 0.0, Slow, CrossChecked, SGBD_CAP_RR),
    ch!("battery_v", "Battery voltage", "V", blk(M, 43, U8), 0.10156, 0.0, Slow, CrossChecked, SGBD_CAP_RR),
    // ---------------- 0B 90 VANOS block ----------------
    ch!("vanos_intake_target", "VANOS intake target", "° crank", blk(V, 4, U8), 0.375, 60.0, Fast, Sgbd, SGBD,
        Some("CAM_SP_IN. Requires an extra 0B90 request per cycle.")),
    ch!("vanos_exhaust_target", "VANOS exhaust target", "° crank", blk(V, 10, U8), -0.375, -60.0, Fast, Sgbd, SGBD,
        Some("CAM_SP_EX. Requires an extra 0B90 request per cycle.")),
    ch!("vanos_intake_actual_b90", "VANOS intake actual (0B90)", "° crank", blk(V, 3, U8), 0.375, 60.0, Fast, Sgbd, SGBD),
    ch!("vanos_exhaust_actual_b90", "VANOS exhaust actual (0B90)", "° crank", blk(V, 9, U8), -0.375, -60.0, Fast, Sgbd, SGBD),
    // ---------------- RAM (06 00), software specific ----------------
    ch!("knock_retard_deg", "Knock retard (Knock1_6)", "°", Source::Memory { addresses: KNOCK_ADDR, dtype: U8 },
        0.375, -48.0, Fast, Community, RR_DEFS, RAM_NOTE),
    ch!("ign_cyl_1", "Ignition cyl 1", "° crank", Source::Memory { addresses: &IGN1, dtype: U8 }, -0.375, 72.0, Fast, Community, RR_DEFS, RAM_NOTE),
    ch!("ign_cyl_2", "Ignition cyl 2", "° crank", Source::Memory { addresses: &IGN2, dtype: U8 }, -0.375, 72.0, Fast, Community, RR_DEFS, RAM_NOTE),
    ch!("ign_cyl_3", "Ignition cyl 3", "° crank", Source::Memory { addresses: &IGN3, dtype: U8 }, -0.375, 72.0, Fast, Community, RR_DEFS, RAM_NOTE),
    ch!("ign_cyl_4", "Ignition cyl 4", "° crank", Source::Memory { addresses: &IGN4, dtype: U8 }, -0.375, 72.0, Fast, Community, RR_DEFS, RAM_NOTE),
    ch!("ign_cyl_5", "Ignition cyl 5", "° crank", Source::Memory { addresses: &IGN5, dtype: U8 }, -0.375, 72.0, Fast, Community, RR_DEFS, RAM_NOTE),
    ch!("ign_cyl_6", "Ignition cyl 6", "° crank", Source::Memory { addresses: &IGN6, dtype: U8 }, -0.375, 72.0, Fast, Community, RR_DEFS, RAM_NOTE),
    ch!("gear", "Gear", "", Source::Memory { addresses: GEAR_ADDR, dtype: U8 }, 1.0, 0.0, Normal, Community, RR_DEFS, RAM_NOTE),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ds2::{fixtures, ADDR_DME};
    use std::collections::HashSet;

    fn capture() -> Response {
        Response::parse(&fixtures::bytes(fixtures::STATUS_0B03_IDLE), ADDR_DME).unwrap()
    }

    fn val(id: &str) -> f64 {
        find(id).unwrap().decode_block(&capture()).unwrap().value
    }

    #[test]
    fn decodes_real_idle_capture() {
        assert_eq!(val("rpm"), 707.0);
        assert_eq!(val("vehicle_speed_kph"), 0.0);
        assert!((val("maf_kg_h") - 14.0).abs() < 1e-9);
        assert!((val("coolant_c") - 92.25).abs() < 1e-9);
        assert!((val("oil_c") - 96.076).abs() < 1e-3);
        assert!((val("ignition_deg") - -3.0).abs() < 1e-9);
        assert!((val("injection_ms") - 2.0907).abs() < 1e-3);
        assert!((val("idle_integrator_pct") - -1.419).abs() < 1e-3);
        assert!((val("vanos_intake_actual") - 118.5).abs() < 1e-9);
        assert!((val("vanos_exhaust_actual") - -105.375).abs() < 1e-9);
        assert!((val("kl15_v") - 13.812).abs() < 1e-3);
        assert!((val("lambda_int_1") - 0.985).abs() < 1e-3);
        assert!((val("load_mg_stroke") - 110.05).abs() < 1e-2);
        assert!((val("baro_hpa") - 988.4).abs() < 0.1);
        assert!((val("battery_v") - 13.609).abs() < 1e-3);
    }

    #[test]
    fn definitions_are_consistent() {
        let mut ids = HashSet::new();
        for c in CHANNELS {
            assert!(ids.insert(c.id), "duplicate id {}", c.id);
            if let Source::Block {
                block: StatusBlock::Measurements,
                offset,
                dtype,
            } = c.source
            {
                // 0B03 response is 45 bytes; data must end before the checksum at index 44.
                assert!(
                    offset >= 3 && offset + dtype.size() <= 44,
                    "{} out of range",
                    c.id
                );
            }
        }
    }

    #[test]
    fn short_block_does_not_decode() {
        let r = Response::parse(&crate::ds2::build_frame(0x12, &[0xA0, 0x02]), ADDR_DME).unwrap();
        assert!(find("rpm").unwrap().decode_block(&r).is_none());
    }
}
