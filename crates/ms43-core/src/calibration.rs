//! Read-only calibration viewer: a MS43 bin + matching TunerPro XDF → decoded tables, with
//! table axes bound to live channels for overlays.
//!
//! Layout facts (verified on a 430069 512K image and the MS4x XDFs):
//! * the 64K calibration region starts at 0x70000 in the 512K flash image
//!   (512K XDF `BASEOFFSET offset="0x70000"`; 64K XDF addresses equal the 512K ones);
//! * the software id string ("430056"/"430069") is at calibration offset 0x42
//!   (found at 0x70042 in the 512K image; RomRaider partial ROM defs use internalidaddress 42).

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::xdf::{
    Embedded, Xdf, XdfAxis, XdfTable, FLAG_COLUMN_MAJOR, FLAG_FLOAT, FLAG_LSB_FIRST, FLAG_SIGNED,
};

pub const CAL_START: usize = 0x70000;
pub const CAL_SIZE: usize = 0x10000;
pub const FULL_SIZE: usize = 0x80000;
const ID_OFFSET: usize = 0x42;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum BinLayout {
    /// 64K calibration region only ("partial read").
    Partial64k,
    /// Full 512K flash image.
    Full512k,
}

#[derive(Debug, Clone, Serialize)]
pub struct BinInfo {
    pub path: PathBuf,
    pub size: usize,
    pub layout: BinLayout,
    /// e.g. "430069", read from the image.
    pub software_version: Option<String>,
}

fn layout_of(size: usize) -> Result<BinLayout, String> {
    match size {
        CAL_SIZE => Ok(BinLayout::Partial64k),
        FULL_SIZE => Ok(BinLayout::Full512k),
        n => Err(format!("unexpected bin size {n} bytes; expected a 64K calibration (65536) or 512K image (524288)")),
    }
}

fn cal_base(layout: BinLayout) -> usize {
    match layout {
        BinLayout::Partial64k => 0,
        BinLayout::Full512k => CAL_START,
    }
}

/// Finds "4300dd" at the software-id position.
pub fn software_version_of(bin: &[u8], layout: BinLayout) -> Option<String> {
    let at = cal_base(layout) + ID_OFFSET;
    let s = bin.get(at..at + 6)?;
    (s.starts_with(b"4300") && s[4..].iter().all(u8::is_ascii_digit))
        .then(|| String::from_utf8_lossy(s).into_owned())
}

pub fn inspect_bin(path: &Path) -> Result<BinInfo, String> {
    let bin = fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let layout = layout_of(bin.len())?;
    Ok(BinInfo {
        path: path.to_path_buf(),
        size: bin.len(),
        layout,
        software_version: software_version_of(&bin, layout),
    })
}

/// "MS430056" → "430056"
pub fn xdf_version(xdf: &Xdf) -> Option<String> {
    let t = xdf.title.as_bytes();
    (0..t.len().saturating_sub(5))
        .find(|&i| t[i..].starts_with(b"4300") && t[i + 4..i + 6].iter().all(u8::is_ascii_digit))
        .map(|i| String::from_utf8_lossy(&t[i..i + 6]).into_owned())
}

// ------------------------------------------------------------------ axis ↔ live channel

/// Siemens variable names (from XDF table titles `ip_<out>__<var>__<var>`) that we log.
/// `exact` = same variable as the BMW SGBD row behind the channel.
struct AxisVar {
    var: &'static str,
    units: &'static str,
    channel: &'static str,
    exact: bool,
    note: &'static str,
}

const AXIS_VARS: &[AxisVar] = &[
    AxisVar {
        var: "n",
        units: "rpm",
        channel: "rpm",
        exact: true,
        note: "N",
    },
    AxisVar {
        var: "n_32",
        units: "rpm",
        channel: "rpm",
        exact: false,
        note: "engine speed at 32 rpm resolution",
    },
    AxisVar {
        var: "maf_mes",
        units: "mg/stk",
        channel: "load_mg_stroke",
        exact: true,
        note: "MAF_MES = STATUS_LAST",
    },
    AxisVar {
        var: "maf",
        units: "mg/stk",
        channel: "load_mg_stroke",
        exact: false,
        note:
            "map load 'maf' vs logged MAF_MES (STATUS_LAST); may be filtered/corrected differently",
    },
    AxisVar {
        var: "maf_kgh",
        units: "kg/h",
        channel: "maf_kg_h",
        exact: true,
        note: "MAF_KGH",
    },
    AxisVar {
        var: "tco",
        units: "°C",
        channel: "coolant_c",
        exact: true,
        note: "TCO",
    },
    AxisVar {
        var: "toil",
        units: "°C",
        channel: "oil_c",
        exact: true,
        note: "TOIL",
    },
    AxisVar {
        var: "tia",
        units: "°C",
        channel: "iat_c",
        exact: true,
        note: "TIA",
    },
    AxisVar {
        var: "vb",
        units: "V",
        channel: "battery_v",
        exact: true,
        note: "VB",
    },
    AxisVar {
        var: "vs",
        units: "km/h",
        channel: "vehicle_speed_kph",
        exact: true,
        note: "VS",
    },
    AxisVar {
        var: "pvs_av",
        units: "°PVS",
        channel: "pedal_deg",
        exact: true,
        note: "PVS_AV",
    },
    AxisVar {
        var: "tps_av",
        units: "°TPS",
        channel: "throttle_deg",
        exact: false,
        note: "tps_av vs logged TPS_AV_PRJ (different scaling in SGBD)",
    },
    AxisVar {
        var: "amp",
        units: "hPa",
        channel: "baro_hpa",
        exact: false,
        note: "amp vs logged AMP_MES",
    },
];

#[derive(Debug, Clone, Serialize)]
pub struct AxisBinding {
    pub var: &'static str,
    pub channel: &'static str,
    pub exact: bool,
    pub note: &'static str,
}

fn find_var(var: &str, units: &str) -> Option<&'static AxisVar> {
    AXIS_VARS
        .iter()
        .find(|v| v.var == var && units.trim().starts_with(v.units))
}

/// Binds x/y axes using the title variables, checked against axis units. Title order is
/// usually `__<y>__<x>`, but not always, so each variable goes to the axis whose units match.
fn bind_axes(t: &XdfTable) -> (Option<AxisBinding>, Option<AxisBinding>) {
    let vars: Vec<&str> = t.title.split("__").skip(1).collect();
    let units = |a: &Option<XdfAxis>| a.as_ref().map(|a| a.units.clone()).unwrap_or_default();
    let (ux, uy) = (units(&t.x), units(&t.y));
    let multi = |a: &Option<XdfAxis>| a.as_ref().is_some_and(|a| a.index_count > 1);
    let bind = |v: &AxisVar| AxisBinding {
        var: v.var,
        channel: v.channel,
        exact: v.exact,
        note: v.note,
    };
    let mut x = None;
    let mut y = None;
    let order: &[(&str, char)] = match vars.len() {
        2 => &[
            (vars[0], 'y'),
            (vars[1], 'x'),
            (vars[0], 'x'),
            (vars[1], 'y'),
        ],
        1 => &[(vars[0], 'x'), (vars[0], 'y')],
        _ => &[],
    };
    for &(var, axis) in order {
        let (taken, u, ok) = if axis == 'x' {
            (x.is_some(), &ux, multi(&t.x))
        } else {
            (y.is_some(), &uy, multi(&t.y))
        };
        if taken || !ok {
            continue;
        }
        let Some(v) = find_var(var, u) else { continue };
        // never bind the same variable to both axes
        let other: &Option<AxisBinding> = if axis == 'x' { &y } else { &x };
        if other.as_ref().map(|b| b.var) == Some(v.var) {
            continue;
        }
        if axis == 'x' {
            x = Some(bind(v));
        } else {
            y = Some(bind(v));
        }
    }
    (x, y)
}

// ------------------------------------------------------------------ decoded tables

#[derive(Debug, Clone, Serialize)]
pub struct TableInfo {
    pub uid: u32,
    pub title: String,
    pub description: String,
    pub category: String,
    pub rows: usize,
    pub cols: usize,
    pub units: String,
    pub x_units: String,
    pub y_units: String,
    pub x_channel: Option<AxisBinding>,
    pub y_channel: Option<AxisBinding>,
    pub constant: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct AxisData {
    pub units: String,
    pub values: Vec<f64>,
    pub binding: Option<AxisBinding>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TableData {
    pub info: TableInfo,
    pub x: AxisData,
    pub y: AxisData,
    /// `values[row][col]`; rows follow the y axis, columns the x axis.
    pub values: Vec<Vec<f64>>,
    pub raw: Vec<Vec<i64>>,
    pub decimals: u32,
    pub equation: String,
    pub address: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CalibrationSummary {
    pub bin: BinInfo,
    pub xdf_path: PathBuf,
    pub xdf_title: String,
    pub xdf_version: Option<String>,
    pub tables: usize,
    pub overlayable: usize,
}

pub struct Calibration {
    bin: Vec<u8>,
    info: BinInfo,
    xdf: Xdf,
    xdf_path: PathBuf,
}

impl Calibration {
    /// Loads and cross-checks bin and XDF. Refuses a pair whose software versions differ.
    pub fn load(bin_path: &Path, xdf_path: &Path) -> Result<Self, String> {
        let bin =
            fs::read(bin_path).map_err(|e| format!("cannot read {}: {e}", bin_path.display()))?;
        let layout = layout_of(bin.len())?;
        let text = fs::read_to_string(xdf_path)
            .or_else(|_| fs::read(xdf_path).map(|b| b.iter().map(|&c| c as char).collect()))
            .map_err(|e| format!("cannot read {}: {e}", xdf_path.display()))?;
        let xdf = Xdf::parse(&text).map_err(|e| format!("{}: {e}", xdf_path.display()))?;
        let info = BinInfo {
            path: bin_path.to_path_buf(),
            size: bin.len(),
            layout,
            software_version: software_version_of(&bin, layout),
        };
        match (&info.software_version, xdf_version(&xdf)) {
            (Some(b), Some(x)) if *b != x => {
                return Err(format!(
                    "calibration is software {b} but the XDF is for {x}; load the {b} XDF"
                ));
            }
            (None, _) => {
                return Err(
                    "no MS43 software id (4300xx) at calibration offset 0x42; is this an MS43 bin?"
                        .into(),
                )
            }
            _ => {}
        }
        Ok(Self {
            bin,
            info,
            xdf,
            xdf_path: xdf_path.to_path_buf(),
        })
    }

    pub fn software_version(&self) -> Option<&str> {
        self.info.software_version.as_deref()
    }

    pub fn summary(&self) -> CalibrationSummary {
        let tables = self.list();
        CalibrationSummary {
            bin: self.info.clone(),
            xdf_path: self.xdf_path.clone(),
            xdf_title: self.xdf.title.clone(),
            xdf_version: xdf_version(&self.xdf),
            overlayable: tables
                .iter()
                .filter(|t| t.x_channel.is_some() || t.y_channel.is_some())
                .count(),
            tables: tables.len(),
        }
    }

    /// All decodable, user-facing tables (the Axis category holds breakpoint vectors only).
    pub fn list(&self) -> Vec<TableInfo> {
        self.xdf
            .tables
            .iter()
            .filter(|t| t.z.data.is_some())
            .filter(|t| self.category(t) != "Axis")
            .map(|t| self.info_of(t))
            .collect()
    }

    fn category(&self, t: &XdfTable) -> String {
        t.categories
            .first()
            .and_then(|&c| self.xdf.category_name(c))
            .unwrap_or("")
            .to_string()
    }

    fn info_of(&self, t: &XdfTable) -> TableInfo {
        let (x_channel, y_channel) = bind_axes(t);
        let z = t.z.data.as_ref();
        TableInfo {
            uid: t.uid,
            title: t.title.clone(),
            description: t.description.clone(),
            category: self.category(t),
            rows: z.map_or(1, |d| d.rows),
            cols: z.map_or(1, |d| d.cols),
            units: t.z.units.clone(),
            x_units: t.x.as_ref().map(|a| a.units.clone()).unwrap_or_default(),
            y_units: t.y.as_ref().map(|a| a.units.clone()).unwrap_or_default(),
            x_channel,
            y_channel,
            constant: t.constant,
        }
    }

    pub fn table(&self, uid: u32) -> Result<TableData, String> {
        let t = self
            .xdf
            .table(uid)
            .ok_or_else(|| format!("no table 0x{uid:X}"))?;
        let d = t.z.data.as_ref().ok_or("table has no data address")?;
        if !t.z.math.is_supported() {
            return Err(format!("unsupported conversion '{}'", t.z.math.source));
        }
        let raw = self.read_block(d)?;
        let values = raw
            .iter()
            .map(|row| {
                row.iter()
                    .map(|&r| t.z.math.eval(r as f64).unwrap_or(f64::NAN))
                    .collect()
            })
            .collect();
        let info = self.info_of(t);
        let x = AxisData {
            units: info.x_units.clone(),
            values: self.axis_values(t.x.as_ref(), d.cols)?,
            binding: info.x_channel.clone(),
        };
        let y = AxisData {
            units: info.y_units.clone(),
            values: self.axis_values(t.y.as_ref(), d.rows)?,
            binding: info.y_channel.clone(),
        };
        Ok(TableData {
            info,
            x,
            y,
            values,
            raw,
            decimals: t.z.decimals.unwrap_or(2),
            equation: t.z.math.source.clone(),
            address: format!("0x{:X}", d.address),
        })
    }

    fn axis_values(&self, axis: Option<&XdfAxis>, n: usize) -> Result<Vec<f64>, String> {
        let index = || (0..n).map(|i| i as f64).collect::<Vec<_>>();
        let Some(a) = axis else { return Ok(index()) };
        let raw: Vec<f64> = if let Some(id) = a.link {
            let lt = self
                .xdf
                .table(id)
                .ok_or_else(|| format!("axis link 0x{id:X} not found"))?;
            let ld =
                lt.z.data
                    .as_ref()
                    .ok_or_else(|| format!("axis 0x{id:X} has no data"))?;
            let vals: Vec<f64> = self
                .read_block(ld)?
                .into_iter()
                .flatten()
                .map(|r| lt.z.math.eval(r as f64).unwrap_or(f64::NAN))
                .collect();
            vals
        } else if let Some(d) = &a.data {
            self.read_block(d)?
                .into_iter()
                .flatten()
                .map(|r| r as f64)
                .collect()
        } else if !a.labels.is_empty() {
            return Ok(a.labels.iter().copied().take(n).collect());
        } else {
            return Ok(index());
        };
        let mut v: Vec<f64> = raw
            .into_iter()
            .map(|r| a.math.eval(r).unwrap_or(f64::NAN))
            .take(n)
            .collect();
        if v.len() < n {
            // definition shorter than the data: pad with indices so the grid still renders
            v.extend((v.len()..n).map(|i| i as f64));
        }
        Ok(v)
    }

    /// Reads a rows×cols block of raw integers.
    fn read_block(&self, d: &Embedded) -> Result<Vec<Vec<i64>>, String> {
        let flags = if d.flags == u32::MAX {
            (if self.xdf.default_signed {
                FLAG_SIGNED
            } else {
                0
            }) | (if self.xdf.default_lsb_first {
                FLAG_LSB_FIRST
            } else {
                0
            })
        } else {
            d.flags
        };
        if flags & FLAG_FLOAT != 0 {
            return Err("floating-point tables are not supported".into());
        }
        if d.major_stride_bits != 0 || d.minor_stride_bits != 0 {
            return Err("strided tables are not supported".into());
        }
        let size = match d.bits {
            8 | 16 | 32 => (d.bits / 8) as usize,
            b => return Err(format!("unsupported element size {b} bits")),
        };
        let base = self.file_offset(d.address)?;
        let end = base + d.rows * d.cols * size;
        if end > self.bin.len() {
            return Err(format!(
                "table at 0x{:X} runs past the end of the bin",
                d.address
            ));
        }
        let mut out = vec![vec![0i64; d.cols]; d.rows];
        for (r, row) in out.iter_mut().enumerate() {
            for (c, cell) in row.iter_mut().enumerate() {
                let i = if flags & FLAG_COLUMN_MAJOR != 0 {
                    c * d.rows + r
                } else {
                    r * d.cols + c
                };
                let b = &self.bin[base + i * size..base + (i + 1) * size];
                let mut v: u64 = 0;
                for k in 0..size {
                    let byte = if flags & FLAG_LSB_FIRST != 0 {
                        b[size - 1 - k]
                    } else {
                        b[k]
                    };
                    v = (v << 8) | byte as u64;
                }
                *cell = if flags & FLAG_SIGNED != 0 {
                    let shift = 64 - 8 * size;
                    ((v << shift) as i64) >> shift
                } else {
                    v as i64
                };
            }
        }
        Ok(out)
    }

    /// XDF address → offset in the loaded bin, whichever layout each was made for.
    fn file_offset(&self, address: u32) -> Result<usize, String> {
        let xdf_region_base: i64 = if self.xdf.region_size == CAL_SIZE as u64 {
            CAL_START as i64
        } else {
            0
        };
        let absolute = address as i64 + self.xdf.base_offset + xdf_region_base;
        // where this bin starts within the 512K flash image
        let bin_start: i64 = match self.info.layout {
            BinLayout::Partial64k => CAL_START as i64,
            BinLayout::Full512k => 0,
        };
        let off = absolute - bin_start;
        usize::try_from(off).map_err(|_| format!("address 0x{address:X} is outside this bin"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Synthetic XDF in the MS4x shape: a 2x3 map with linked rpm and load axes.
    const XDF: &str = r#"<!-- test -->
<XDFFORMAT version="1.80">
  <XDFHEADER>
    <deftitle>MS430069</deftitle>
    <BASEOFFSET offset="0" subtract="0" />
    <DEFAULTS datasizeinbits="8" sigdigits="2" outputtype="1" signed="0" lsbfirst="0" float="0" />
    <REGION type="0xFFFFFFFF" startaddress="0x0" size="0x10000" />
    <CATEGORY index="0x0" name="Axis" />
    <CATEGORY index="0x88" name="Ignition" />
  </XDFHEADER>
  <XDFTABLE uniqueid="0x100" flags="0x30">
    <title>ip_test__n__maf</title>
    <description>test map</description>
    <CATEGORYMEM index="0" category="137" />
    <XDFAXIS id="x" uniqueid="0x0"><units>mg/stk</units><indexcount>3</indexcount><embedinfo type="3" linkobjid="0x200" /><MATH equation="X"><VAR id="X" /></MATH></XDFAXIS>
    <XDFAXIS id="y" uniqueid="0x0"><units>rpm</units><indexcount>2</indexcount><embedinfo type="3" linkobjid="0x201" /><MATH equation="X"><VAR id="X" /></MATH></XDFAXIS>
    <XDFAXIS id="z">
      <EMBEDDEDDATA mmedtypeflags="0x02" mmedaddress="0x1000" mmedelementsizebits="8" mmedrowcount="2" mmedcolcount="3" mmedmajorstridebits="0" mmedminorstridebits="0" />
      <units>&#xB0;CRK</units><decimalpl>1</decimalpl>
      <MATH equation="0.375*X-23.625"><VAR id="X" /></MATH>
    </XDFAXIS>
  </XDFTABLE>
  <XDFTABLE uniqueid="0x200" flags="0x30">
    <title>ldpm_maf_3</title><CATEGORYMEM index="0" category="1" />
    <XDFAXIS id="z"><EMBEDDEDDATA mmedtypeflags="0x02" mmedaddress="0x2000" mmedelementsizebits="16" mmedrowcount="3" /><MATH equation="0.021194781*X"><VAR id="X" /></MATH></XDFAXIS>
  </XDFTABLE>
  <XDFTABLE uniqueid="0x201" flags="0x30">
    <title>ldpm_n_2</title><CATEGORYMEM index="0" category="1" />
    <XDFAXIS id="z"><EMBEDDEDDATA mmedtypeflags="0x02" mmedaddress="0x2010" mmedelementsizebits="8" mmedrowcount="2" /><MATH equation="32.0*X"><VAR id="X" /></MATH></XDFAXIS>
  </XDFTABLE>
</XDFFORMAT>"#;

    fn bin(layout: BinLayout) -> Vec<u8> {
        let mut cal = vec![0u8; CAL_SIZE];
        cal[ID_OFFSET..ID_OFFSET + 6].copy_from_slice(b"430069");
        cal[0x1000..0x1006].copy_from_slice(&[64, 80, 96, 65, 81, 97]); // row-major 2x3
        for (i, v) in [1000u16, 2000, 3000].iter().enumerate() {
            cal[0x2000 + 2 * i..0x2002 + 2 * i].copy_from_slice(&v.to_le_bytes());
        }
        cal[0x2010..0x2012].copy_from_slice(&[25, 200]); // 800, 6400 rpm
        match layout {
            BinLayout::Partial64k => cal,
            BinLayout::Full512k => {
                let mut full = vec![0xFFu8; FULL_SIZE];
                full[CAL_START..].copy_from_slice(&cal);
                full
            }
        }
    }

    fn load(layout: BinLayout, xdf: &str) -> Result<Calibration, String> {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("logger43-cal-{}-{n}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("t.bin"), bin(layout)).unwrap();
        fs::write(dir.join("t.xdf"), xdf).unwrap();
        Calibration::load(&dir.join("t.bin"), &dir.join("t.xdf"))
    }

    #[test]
    fn decodes_map_axes_and_bindings_in_both_layouts() {
        for layout in [BinLayout::Partial64k, BinLayout::Full512k] {
            let cal = load(layout, XDF).unwrap();
            assert_eq!(cal.software_version(), Some("430069"));
            let list = cal.list();
            assert_eq!(list.len(), 1, "axis tables are hidden");
            assert_eq!(list[0].category, "Ignition");
            let t = cal.table(0x100).unwrap();
            assert_eq!(
                t.values[0],
                vec![
                    0.375 * 64.0 - 23.625,
                    0.375 * 80.0 - 23.625,
                    0.375 * 96.0 - 23.625
                ]
            );
            assert_eq!(t.y.values, vec![800.0, 6400.0]);
            assert!((t.x.values[2] - 3000.0 * 0.021194781).abs() < 1e-9);
            assert_eq!(t.y.binding.as_ref().unwrap().channel, "rpm");
            assert_eq!(t.x.binding.as_ref().unwrap().channel, "load_mg_stroke");
            assert!(!t.x.binding.as_ref().unwrap().exact, "'maf' is approximate");
        }
    }

    #[test]
    fn refuses_mismatched_versions() {
        let e = load(BinLayout::Partial64k, &XDF.replace("MS430069", "MS430056"))
            .err()
            .unwrap();
        assert!(e.contains("430069") && e.contains("430056"), "{e}");
    }

    #[test]
    fn little_endian_signed_and_column_major() {
        let mut cal = load(BinLayout::Partial64k, XDF).unwrap();
        cal.bin[0x3000..0x3004].copy_from_slice(&[0xFE, 0xFF, 0x02, 0x00]); // -2, 2 LE
        let d = Embedded {
            address: 0x3000,
            bits: 16,
            rows: 1,
            cols: 2,
            flags: 0x03,
            major_stride_bits: 0,
            minor_stride_bits: 0,
        };
        assert_eq!(cal.read_block(&d).unwrap(), vec![vec![-2, 2]]);
        cal.bin[0x3010..0x3014].copy_from_slice(&[1, 2, 3, 4]);
        let d = Embedded {
            address: 0x3010,
            bits: 8,
            rows: 2,
            cols: 2,
            flags: 0x06,
            major_stride_bits: 0,
            minor_stride_bits: 0,
        };
        assert_eq!(cal.read_block(&d).unwrap(), vec![vec![1, 3], vec![2, 4]]);
    }

    /// Runs against your real files: LOGGER43_XDF=... LOGGER43_BIN=... cargo test -- --ignored
    #[test]
    #[ignore]
    fn real_files() {
        let (Some(x), Some(b)) = (
            std::env::var_os("LOGGER43_XDF"),
            std::env::var_os("LOGGER43_BIN"),
        ) else {
            panic!("set LOGGER43_XDF and LOGGER43_BIN");
        };
        let cal = Calibration::load(Path::new(&b), Path::new(&x)).unwrap();
        let s = cal.summary();
        println!("{s:#?}");
        let list = cal.list();
        let mut failures = 0;
        for t in &list {
            if let Err(e) = cal.table(t.uid) {
                failures += 1;
                println!("FAIL {}: {e}", t.title);
            }
        }
        println!("{} tables, {} failed to decode", list.len(), failures);
        assert_eq!(failures, 0);
    }
}
