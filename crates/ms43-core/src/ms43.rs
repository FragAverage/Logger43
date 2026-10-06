//! MS43-specific knowledge: identification layout and which DME part numbers are known MS43.

use serde::Serialize;

use crate::ds2::Response;

/// BMW part numbers (`ID_BMW_NR`) known to be MS43. Sources: Logger.S `ecuList`
/// (handmade0octopus/ds2) and RomRaider MS43 logger definitions (Rustle333/Siemens-MS43).
pub const KNOWN_MS43_PART_NUMBERS: &[(&str, &str)] = &[
    (
        "7551615",
        "MS43 software 430069 (RomRaider/MS4X reference release)",
    ),
    ("7545150", "MS43 (RomRaider/Logger.S list)"),
    ("7519308", "MS43 (RomRaider/Logger.S list)"),
    ("7511570", "MS43 (Logger.S list; RomRaider ADC/switch defs)"),
];

/// MS43 software version for a part number, from the RomRaider ROM definitions' `ecuid`
/// (`430069` ↔ 7551615, `430056` ↔ 7519308). Other part numbers: unknown.
pub fn software_version(part_number: &str) -> Option<&'static str> {
    match part_number {
        "7551615" => Some("430069"),
        "7519308" => Some("430056"),
        _ => None,
    }
}

/// Earlier Siemens DMEs whose `0B 03` layout differs (Logger.S `ecuList`). Used for a clear warning.
pub const KNOWN_NON_MS43_PART_NUMBERS: &[&str] = &["1429764", "1430844", "7526753", "7500255"];

#[derive(Debug, Clone, Default, Serialize)]
pub struct EcuIdent {
    pub bmw_part_number: Option<String>,
    pub hardware_number: Option<String>,
    pub coding_index: Option<String>,
    pub diag_index: Option<String>,
    pub bus_index: Option<String>,
    pub production_week: Option<String>,
    pub production_year: Option<String>,
    pub supplier_number: Option<String>,
    pub software_number: Option<String>,
    pub change_index: Option<String>,
    pub production_number: Option<String>,
    /// Data bytes after the status byte, for display/debug.
    pub raw_ascii: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum LayoutTrust {
    /// Part number is on the known MS43 list: MS430DS0 block layouts apply.
    KnownMs43,
    /// Part number belongs to a different Siemens DME: MS43 layouts must not be used.
    OtherDme,
    /// Not on any list. Layout probably applies to an MS43 but is unconfirmed for this part number.
    Unknown,
}

impl EcuIdent {
    /// Field offsets are frame byte offsets taken from the MS430DS0 `IDENT` job (docs §6).
    pub fn parse(resp: &Response) -> Self {
        let b = resp.bytes();
        let field = |offset: usize, len: usize| -> Option<String> {
            let end = offset + len;
            // never read into the checksum byte
            (end < b.len()).then(|| ascii(&b[offset..end]).trim().to_string())
        };
        Self {
            bmw_part_number: field(3, 7),
            hardware_number: field(10, 2),
            coding_index: field(12, 2),
            diag_index: field(14, 2),
            bus_index: field(16, 2),
            production_week: field(18, 2),
            production_year: field(20, 2),
            supplier_number: field(22, 10),
            software_number: field(32, 2),
            change_index: field(34, 2),
            production_number: field(37, 8),
            raw_ascii: ascii(resp.data()),
        }
    }

    pub fn layout_trust(&self) -> LayoutTrust {
        let Some(pn) = self.bmw_part_number.as_deref() else {
            return LayoutTrust::Unknown;
        };
        if KNOWN_MS43_PART_NUMBERS.iter().any(|(p, _)| *p == pn) {
            LayoutTrust::KnownMs43
        } else if KNOWN_NON_MS43_PART_NUMBERS.contains(&pn) {
            LayoutTrust::OtherDme
        } else {
            LayoutTrust::Unknown
        }
    }

    pub fn description(&self) -> Option<&'static str> {
        let pn = self.bmw_part_number.as_deref()?;
        KNOWN_MS43_PART_NUMBERS
            .iter()
            .find(|(p, _)| *p == pn)
            .map(|(_, d)| *d)
    }
}

fn ascii(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&c| {
            if (0x20..0x7F).contains(&c) {
                c as char
            } else {
                '.'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ds2::{fixtures, ADDR_DME};

    #[test]
    fn parses_romraider_ident_sample() {
        let r = Response::parse(&fixtures::bytes(fixtures::IDENT_ROMRAIDER), ADDR_DME).unwrap();
        let id = EcuIdent::parse(&r);
        assert_eq!(id.bmw_part_number.as_deref(), Some("1437806"));
        assert_eq!(id.hardware_number.as_deref(), Some("11"));
        assert_eq!(id.coding_index.as_deref(), Some("01"));
        assert_eq!(id.production_year.as_deref(), Some("96"));
        // 1437806 is not on either list
        assert_eq!(id.layout_trust(), LayoutTrust::Unknown);
    }
}
