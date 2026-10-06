//! BMW DS2 telegram framing.
//!
//! Frame: `[address] [total length] [payload ...] [xor checksum]`, where the length byte counts
//! the whole frame and the checksum is the XOR of every preceding byte. Responses carry a status
//! byte as the first payload byte (`0xA0` = OK). See `docs/PROTOCOL.md` §2.

use serde::Serialize;
use thiserror::Error;

/// DS2 address of the DME (MS43). Sources: MS430DS0 SGBD, RomRaider, EdiabasLib, Logger.S.
pub const ADDR_DME: u8 = 0x12;
/// Positive response status byte (MS430DS0 `JOBRESULT`).
pub const STATUS_OK: u8 = 0xA0;
/// Smallest legal frame: address, length, one payload/status byte, checksum.
pub const MIN_FRAME_LEN: usize = 4;
/// Length is a single byte, so this is the protocol maximum.
pub const MAX_FRAME_LEN: usize = 255;

pub fn checksum(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0, |acc, b| acc ^ b)
}

/// Builds a complete frame. Callers outside this crate go through `safety::Request`.
pub(crate) fn build_frame(address: u8, payload: &[u8]) -> Vec<u8> {
    let len = payload.len() + 3;
    assert!(len <= MAX_FRAME_LEN, "DS2 frame too long");
    let mut frame = Vec::with_capacity(len);
    frame.push(address);
    frame.push(len as u8);
    frame.extend_from_slice(payload);
    frame.push(checksum(&frame));
    frame
}

#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize)]
pub enum FrameError {
    #[error("frame too short ({0} bytes)")]
    TooShort(usize),
    #[error("length byte {declared} does not match frame size {actual}")]
    LengthMismatch { declared: usize, actual: usize },
    #[error("unexpected address 0x{got:02X} (expected 0x{expected:02X})")]
    WrongAddress { expected: u8, got: u8 },
    #[error("checksum mismatch: frame has 0x{in_frame:02X}, computed 0x{computed:02X}")]
    Checksum { in_frame: u8, computed: u8 },
}

/// A complete, checksum-valid response telegram.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    bytes: Vec<u8>,
}

impl Response {
    /// Validates address, length and checksum. The status byte is checked separately so a
    /// well-formed negative response can still be reported with its BMW status text.
    pub fn parse(bytes: &[u8], expected_address: u8) -> Result<Self, FrameError> {
        if bytes.len() < MIN_FRAME_LEN {
            return Err(FrameError::TooShort(bytes.len()));
        }
        if bytes[0] != expected_address {
            return Err(FrameError::WrongAddress {
                expected: expected_address,
                got: bytes[0],
            });
        }
        let declared = bytes[1] as usize;
        if declared != bytes.len() {
            return Err(FrameError::LengthMismatch {
                declared,
                actual: bytes.len(),
            });
        }
        let computed = checksum(&bytes[..bytes.len() - 1]);
        let in_frame = bytes[bytes.len() - 1];
        if computed != in_frame {
            return Err(FrameError::Checksum { in_frame, computed });
        }
        Ok(Self {
            bytes: bytes.to_vec(),
        })
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn status(&self) -> u8 {
        self.bytes[2]
    }

    pub fn is_ok(&self) -> bool {
        self.status() == STATUS_OK
    }

    /// Data after the status byte, excluding the checksum.
    pub fn data(&self) -> &[u8] {
        &self.bytes[3..self.bytes.len() - 1]
    }
}

/// Status byte text from the MS430DS0 `JOBRESULT` table.
pub fn status_text(status: u8) -> &'static str {
    match status {
        0xA0 => "OKAY",
        0xA1 => "BUSY",
        0xA2 => "ERROR_ECU_REJECTED",
        0xB0 => "ERROR_ECU_PARAMETER",
        0xB1 => "ERROR_ECU_FUNCTION",
        0xB2 => "ERROR_ECU_NUMBER",
        0xFF => "ERROR_ECU_NACK",
        0x11 => "AIF_NICHT_PROGRAMMIERT",
        0x01 => "ERROR_ECU_FUNCTION_ANSTEUERBEDINGUNG_NICHT_ERFUELLT",
        0x02 => "ERROR_ECU_FUNCTION_UEBERGABEPARAMETER_UNGUELTIG",
        0x05 => "ERROR_ECU_FUNCTION NOCH NICHT GESTARTET",
        0x06 => "FUNCTION BEENDET; DATEN GUELTIG",
        _ => "ERROR_ECU_UNKNOWN_STATUSBYTE",
    }
}

/// Formats bytes as `12 05 0B 03 1F`.
pub fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 {
            s.push(' ');
        }
        s.push_str(&format!("{b:02X}"));
    }
    s
}

#[cfg(test)]
pub(crate) mod fixtures {
    /// Real MS43 (E46 330i, warm idle) response to `12 05 0B 03 1F`, from tomicooler/bmwe46oil.
    pub const STATUS_0B03_IDLE: &str =
        "122da002c30000000038003899bbb5a7c80188fc4e64359c79887c2f7c55fefefefe1447097605050d2e9086c2";
    /// Real DS2 ident response quoted in RomRaider's DS2Protocol.java.
    pub const IDENT_ROMRAIDER: &str = "122ea031343337383036\
3131303133303231323239363030303031313538353236303030393632313432353634\
9c";

    pub fn bytes(hex: &str) -> Vec<u8> {
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_request_checksums() {
        assert_eq!(build_frame(0x12, &[0x00]), vec![0x12, 0x04, 0x00, 0x16]);
        assert_eq!(
            build_frame(0x12, &[0x0B, 0x03]),
            vec![0x12, 0x05, 0x0B, 0x03, 0x1F]
        );
    }

    #[test]
    fn real_captures_parse() {
        let r = Response::parse(&fixtures::bytes(fixtures::STATUS_0B03_IDLE), ADDR_DME).unwrap();
        assert!(r.is_ok());
        assert_eq!(r.bytes().len(), 45);
        let r = Response::parse(&fixtures::bytes(fixtures::IDENT_ROMRAIDER), ADDR_DME).unwrap();
        assert_eq!(&r.data()[..7], b"1437806");
    }

    #[test]
    fn rejects_corruption() {
        let mut b = fixtures::bytes(fixtures::STATUS_0B03_IDLE);
        b[10] ^= 0x01;
        assert!(matches!(
            Response::parse(&b, ADDR_DME),
            Err(FrameError::Checksum { .. })
        ));
        let b = fixtures::bytes(fixtures::STATUS_0B03_IDLE);
        assert!(matches!(
            Response::parse(&b[..44], ADDR_DME),
            Err(FrameError::LengthMismatch { .. })
        ));
        assert!(matches!(
            Response::parse(&b, 0x13),
            Err(FrameError::WrongAddress { .. })
        ));
    }
}
