//! TunerPro XDF (XML definition) parser, limited to what the MS4x MS43 XDFs use.
//!
//! `mmedtypeflags` bits, as interpreted by TunerPro-compatible readers (UniversalPatcher
//! `xdf.cs`, romHEX14 `XdfIo.cpp`, MxT) and confirmed on real MS43 data (205/211 16-bit axes
//! are strictly increasing when read LSB-first): 0x01 signed, 0x02 LSB first (little-endian),
//! 0x04 column-major, 0x10000 IEEE float. Anything this parser cannot decode exactly is marked
//! unsupported rather than approximated.

use std::collections::HashMap;

use serde::Serialize;
use thiserror::Error;

pub const FLAG_SIGNED: u32 = 0x01;
pub const FLAG_LSB_FIRST: u32 = 0x02;
pub const FLAG_COLUMN_MAJOR: u32 = 0x04;
pub const FLAG_FLOAT: u32 = 0x10000;

#[derive(Debug, Error)]
pub enum XdfError {
    #[error("XML error: {0}")]
    Xml(#[from] roxmltree::Error),
    #[error("not a TunerPro XDF (no XDFFORMAT root)")]
    NotXdf,
}

#[derive(Debug, Clone, Serialize)]
pub struct Embedded {
    pub address: u32,
    pub bits: u32,
    pub rows: usize,
    pub cols: usize,
    pub flags: u32,
    pub major_stride_bits: i32,
    pub minor_stride_bits: i32,
}

#[derive(Debug, Clone)]
pub struct XdfAxis {
    pub units: String,
    pub index_count: usize,
    /// Data stored in the binary (with an address).
    pub data: Option<Embedded>,
    /// `embedinfo type="3"`: breakpoints are the data of another table.
    pub link: Option<u32>,
    pub labels: Vec<f64>,
    pub math: Math,
    pub decimals: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct XdfTable {
    pub uid: u32,
    pub title: String,
    pub description: String,
    pub categories: Vec<u32>,
    pub x: Option<XdfAxis>,
    pub y: Option<XdfAxis>,
    pub z: XdfAxis,
    /// XDFCONSTANT (single value) rather than XDFTABLE.
    pub constant: bool,
}

#[derive(Debug, Clone)]
pub struct Xdf {
    pub title: String,
    pub description: String,
    /// Added to every XDF address to get the offset in the bin it was written for.
    pub base_offset: i64,
    /// Size of the bin the XDF was written for (REGION size).
    pub region_size: u64,
    pub default_signed: bool,
    pub default_lsb_first: bool,
    pub categories: HashMap<u32, String>,
    pub tables: Vec<XdfTable>,
    by_uid: HashMap<u32, usize>,
}

impl Xdf {
    pub fn parse(text: &str) -> Result<Self, XdfError> {
        let doc = roxmltree::Document::parse_with_options(
            text,
            roxmltree::ParsingOptions {
                allow_dtd: true,
                ..Default::default()
            },
        )?;
        let root = doc.root_element();
        if !root.has_tag_name("XDFFORMAT") {
            return Err(XdfError::NotXdf);
        }
        let mut xdf = Xdf {
            title: String::new(),
            description: String::new(),
            base_offset: 0,
            region_size: 0,
            default_signed: false,
            default_lsb_first: false,
            categories: HashMap::new(),
            tables: Vec::new(),
            by_uid: HashMap::new(),
        };
        if let Some(h) = child(root, "XDFHEADER") {
            xdf.title = child_text(h, "deftitle");
            xdf.description = child_text(h, "description");
            if let Some(b) = child(h, "BASEOFFSET") {
                let off = num(b.attribute("offset")).unwrap_or(0);
                let sub = num(b.attribute("subtract")).unwrap_or(0) != 0;
                xdf.base_offset = if sub { -off } else { off };
            }
            if let Some(d) = child(h, "DEFAULTS") {
                xdf.default_signed = num(d.attribute("signed")).unwrap_or(0) != 0;
                xdf.default_lsb_first = num(d.attribute("lsbfirst")).unwrap_or(0) != 0;
            }
            if let Some(r) = child(h, "REGION") {
                xdf.region_size = num(r.attribute("size")).unwrap_or(0) as u64;
            }
            for c in h.children().filter(|n| n.has_tag_name("CATEGORY")) {
                if let (Some(i), Some(name)) = (num(c.attribute("index")), c.attribute("name")) {
                    xdf.categories.insert(i as u32, name.to_string());
                }
            }
        }
        for node in root.children().filter(|n| n.is_element()) {
            let constant = node.has_tag_name("XDFCONSTANT");
            if !(constant || node.has_tag_name("XDFTABLE")) {
                continue;
            }
            let Some(uid) = num(node.attribute("uniqueid")) else {
                continue;
            };
            let categories = node
                .children()
                .filter(|n| n.has_tag_name("CATEGORYMEM"))
                .filter_map(|n| num(n.attribute("category")))
                // CATEGORYMEM category numbers are 1-based against CATEGORY index
                .map(|c| (c as u32).saturating_sub(1))
                .collect();
            let (x, y, z) = if constant {
                (None, None, parse_axis(node))
            } else {
                let mut axes: HashMap<&str, XdfAxis> = HashMap::new();
                for a in node.children().filter(|n| n.has_tag_name("XDFAXIS")) {
                    if let Some(id) = a.attribute("id") {
                        axes.insert(id, parse_axis(a));
                    }
                }
                let Some(z) = axes.remove("z") else { continue };
                (axes.remove("x"), axes.remove("y"), z)
            };
            xdf.by_uid.insert(uid as u32, xdf.tables.len());
            xdf.tables.push(XdfTable {
                uid: uid as u32,
                title: child_text(node, "title"),
                description: child_text(node, "description"),
                categories,
                x,
                y,
                z,
                constant,
            });
        }
        Ok(xdf)
    }

    pub fn table(&self, uid: u32) -> Option<&XdfTable> {
        self.by_uid.get(&uid).map(|&i| &self.tables[i])
    }

    /// `index` is already CATEGORYMEM − 1, which equals the CATEGORY `index` attribute
    /// (verified: ignition maps carry category="137" and CATEGORY index="0x88" is "Ignition").
    pub fn category_name(&self, index: u32) -> Option<&str> {
        self.categories.get(&index).map(|s| s.as_str())
    }
}

/// Parses an XDFAXIS element, or the body of an XDFCONSTANT (same child layout).
fn parse_axis(node: roxmltree::Node) -> XdfAxis {
    let data = child(node, "EMBEDDEDDATA").and_then(|e| {
        let address = num(e.attribute("mmedaddress"))? as u32;
        Some(Embedded {
            address,
            bits: num(e.attribute("mmedelementsizebits")).unwrap_or(8) as u32,
            rows: num(e.attribute("mmedrowcount")).unwrap_or(1).max(1) as usize,
            cols: num(e.attribute("mmedcolcount")).unwrap_or(1).max(1) as usize,
            flags: num(e.attribute("mmedtypeflags")).unwrap_or(-1) as u32,
            major_stride_bits: num(e.attribute("mmedmajorstridebits")).unwrap_or(0) as i32,
            minor_stride_bits: num(e.attribute("mmedminorstridebits")).unwrap_or(0) as i32,
        })
    });
    let link = child(node, "embedinfo")
        .filter(|e| e.attribute("type") == Some("3"))
        .and_then(|e| num(e.attribute("linkobjid")))
        .map(|v| v as u32);
    let mut labels: Vec<(usize, f64)> = node
        .children()
        .filter(|n| n.has_tag_name("LABEL"))
        .filter_map(|n| {
            Some((
                num(n.attribute("index"))? as usize,
                n.attribute("value")?.trim().parse().ok()?,
            ))
        })
        .collect();
    labels.sort_by_key(|(i, _)| *i);
    let math = child(node, "MATH")
        .map(|m| {
            let var = child(m, "VAR")
                .and_then(|v| v.attribute("id"))
                .unwrap_or("X")
                .to_string();
            Math::parse(m.attribute("equation").unwrap_or("X"), &var)
        })
        .unwrap_or_else(Math::identity);
    XdfAxis {
        units: child_text(node, "units"),
        index_count: child_text(node, "indexcount").parse().unwrap_or(0),
        data,
        link,
        labels: labels.into_iter().map(|(_, v)| v).collect(),
        math,
        decimals: child_text(node, "decimalpl").parse().ok(),
    }
}

fn child<'a, 'i>(n: roxmltree::Node<'a, 'i>, tag: &str) -> Option<roxmltree::Node<'a, 'i>> {
    n.children().find(|c| c.has_tag_name(tag))
}

fn child_text(n: roxmltree::Node, tag: &str) -> String {
    child(n, tag)
        .and_then(|c| c.text())
        .unwrap_or("")
        .trim()
        .to_string()
}

/// Parses `0x1A`, `26`, `-32`.
fn num(s: Option<&str>) -> Option<i64> {
    let s = s?.trim();
    let (neg, s) = s.strip_prefix('-').map_or((false, s), |r| (true, r));
    let v = if let Some(h) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        i64::from_str_radix(h, 16).ok()?
    } else {
        s.parse().ok()?
    };
    Some(if neg { -v } else { v })
}

// ------------------------------------------------------------------ conversion formulas

#[derive(Debug, Clone)]
enum Expr {
    Num(f64),
    Var,
    Neg(Box<Expr>),
    Bin(Box<Expr>, char, Box<Expr>),
}

/// A TunerPro MATH equation in one variable: + - * / parentheses and numbers.
#[derive(Debug, Clone)]
pub struct Math {
    pub source: String,
    expr: Option<Expr>,
}

impl Math {
    pub fn identity() -> Self {
        Self {
            source: "X".into(),
            expr: Some(Expr::Var),
        }
    }

    pub fn parse(equation: &str, var: &str) -> Self {
        let mut p = ExprParser {
            s: equation.as_bytes(),
            i: 0,
            var: var.as_bytes(),
        };
        let expr = p.expr().filter(|_| {
            p.skip_ws();
            p.i == p.s.len()
        });
        Self {
            source: equation.to_string(),
            expr,
        }
    }

    pub fn is_supported(&self) -> bool {
        self.expr.is_some()
    }

    pub fn eval(&self, x: f64) -> Option<f64> {
        fn ev(e: &Expr, x: f64) -> f64 {
            match e {
                Expr::Num(n) => *n,
                Expr::Var => x,
                Expr::Neg(a) => -ev(a, x),
                Expr::Bin(a, op, b) => {
                    let (a, b) = (ev(a, x), ev(b, x));
                    match op {
                        '+' => a + b,
                        '-' => a - b,
                        '*' => a * b,
                        _ => a / b,
                    }
                }
            }
        }
        self.expr.as_ref().map(|e| ev(e, x))
    }
}

struct ExprParser<'a> {
    s: &'a [u8],
    i: usize,
    var: &'a [u8],
}

impl ExprParser<'_> {
    fn skip_ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }
    fn peek(&mut self) -> Option<u8> {
        self.skip_ws();
        self.s.get(self.i).copied()
    }
    fn expr(&mut self) -> Option<Expr> {
        let mut lhs = self.term()?;
        while let Some(op @ (b'+' | b'-')) = self.peek() {
            self.i += 1;
            lhs = Expr::Bin(Box::new(lhs), op as char, Box::new(self.term()?));
        }
        Some(lhs)
    }
    fn term(&mut self) -> Option<Expr> {
        let mut lhs = self.factor()?;
        while let Some(op @ (b'*' | b'/')) = self.peek() {
            self.i += 1;
            lhs = Expr::Bin(Box::new(lhs), op as char, Box::new(self.factor()?));
        }
        Some(lhs)
    }
    fn factor(&mut self) -> Option<Expr> {
        match self.peek()? {
            b'-' => {
                self.i += 1;
                Some(Expr::Neg(Box::new(self.factor()?)))
            }
            b'+' => {
                self.i += 1;
                self.factor()
            }
            b'(' => {
                self.i += 1;
                let e = self.expr()?;
                (self.peek()? == b')').then(|| self.i += 1)?;
                Some(e)
            }
            c if c.is_ascii_digit() || c == b'.' => {
                let start = self.i;
                while self.i < self.s.len()
                    && (self.s[self.i].is_ascii_digit()
                        || self.s[self.i] == b'.'
                        || ((self.s[self.i] == b'e' || self.s[self.i] == b'E') && self.i > start)
                        || ((self.s[self.i] == b'-' || self.s[self.i] == b'+')
                            && matches!(self.s[self.i - 1], b'e' | b'E')))
                {
                    self.i += 1;
                }
                std::str::from_utf8(&self.s[start..self.i])
                    .ok()?
                    .parse()
                    .ok()
                    .map(Expr::Num)
            }
            _ => {
                // the declared variable (TunerPro accepts it case-insensitively)
                let end = self.i + self.var.len();
                if end <= self.s.len() && self.s[self.i..end].eq_ignore_ascii_case(self.var) {
                    self.i = end;
                    Some(Expr::Var)
                } else {
                    None
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn math_forms_used_by_ms43_xdfs() {
        let cases = [
            ("X", "X", 10.0, 10.0),
            ("0.375*X-23.625", "X", 100.0, 13.875),
            ("-0.375*X-60", "X", 121.0, -105.375),
            ("32.0*X", "X", 25.0, 800.0),
            ("0.1*X000", "X000", 50.0, 5.0),
            ("X*0.003906", "X", 256.0, 0.999936),
            ("x", "x", 3.0, 3.0),
            ("(X-128)*0.375", "X", 128.0, 0.0),
            ("X/2.56+1e-3", "X", 256.0, 100.001),
        ];
        for (eq, var, x, want) in cases {
            let m = Math::parse(eq, var);
            assert!(m.is_supported(), "{eq}");
            assert!(
                (m.eval(x).unwrap() - want).abs() < 1e-9,
                "{eq}: {:?}",
                m.eval(x)
            );
        }
        assert!(!Math::parse("sin(X)", "X").is_supported());
        assert!(!Math::parse("X*Y", "X").is_supported());
    }
}
