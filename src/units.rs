//! Lengths and colors as scripts write them.
//!
//! Documents are measured in CSS px (1/96 in). A render's `scale` converts doc
//! px to device px, so the same file renders as a preview or a print.

use anyhow::{Result, bail};
use vello::peniko::Color;

const UNITS: &[(&str, f64)] = &[
    ("px", 1.0),
    ("pt", 96.0 / 72.0),
    ("mm", 96.0 / 25.4),
    ("cm", 96.0 / 2.54),
    ("in", 96.0),
];

/// Parses "12mm", "9pt", "40" into doc px.
pub fn parse_length(s: &str) -> Result<f64> {
    let s = s.trim();
    let (num, unit) = match UNITS.iter().find(|(u, _)| s.ends_with(u)) {
        Some((u, k)) => (&s[..s.len() - u.len()], *k),
        None => (s, 1.0),
    };
    match num.trim().parse::<f64>() {
        Ok(v) => Ok(v * unit),
        Err(_) => bail!("bad length {s:?} (use a number or e.g. \"12mm\", \"9pt\", \"1in\")"),
    }
}

/// Straight-alpha RGBA in 0..1.
pub type Rgba = [f32; 4];

pub fn parse_color(s: &str) -> Result<Rgba> {
    let s = s.trim();
    match s {
        "none" | "transparent" => return Ok([0.0; 4]),
        "black" => return Ok([0.0, 0.0, 0.0, 1.0]),
        "white" => return Ok([1.0; 4]),
        _ => {}
    }
    if let Some(hex) = s.strip_prefix('#') {
        let digits: Vec<u8> = hex
            .chars()
            .map(|c| c.to_digit(16).map(|d| d as u8))
            .collect::<Option<_>>()
            .unwrap_or_default();
        let n = |hi: u8, lo: u8| (hi * 16 + lo) as f32 / 255.0;
        return match digits.len() {
            3 | 4 => {
                let a = digits.get(3).map_or(1.0, |&d| n(d, d));
                Ok([n(digits[0], digits[0]), n(digits[1], digits[1]), n(digits[2], digits[2]), a])
            }
            6 | 8 => {
                let a = if digits.len() == 8 { n(digits[6], digits[7]) } else { 1.0 };
                Ok([n(digits[0], digits[1]), n(digits[2], digits[3]), n(digits[4], digits[5]), a])
            }
            _ => bail!("bad color {s:?} (use #rgb, #rgba, #rrggbb or #rrggbbaa)"),
        };
    }
    if let Some(body) = s.strip_prefix("rgba(").or_else(|| s.strip_prefix("rgb(")) {
        let parts: Vec<f32> = body
            .trim_end_matches(')')
            .split(',')
            .map(|p| p.trim().parse::<f32>())
            .collect::<Result<_, _>>()
            .map_err(|_| anyhow::anyhow!("bad color {s:?}"))?;
        if parts.len() == 3 || parts.len() == 4 {
            return Ok([parts[0] / 255.0, parts[1] / 255.0, parts[2] / 255.0, *parts.get(3).unwrap_or(&1.0)]);
        }
    }
    bail!("bad color {s:?} (use #hex, rgb(r,g,b), rgba(r,g,b,a) or none)")
}

pub fn to_color(c: Rgba) -> Color {
    Color::new(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths() {
        assert_eq!(parse_length("10").unwrap(), 10.0);
        assert!((parse_length("1in").unwrap() - 96.0).abs() < 1e-9);
        assert!((parse_length("25.4mm").unwrap() - 96.0).abs() < 1e-9);
        assert!(parse_length("3em").is_err());
    }

    #[test]
    fn colors() {
        assert_eq!(parse_color("#fff").unwrap(), [1.0; 4]);
        assert_eq!(parse_color("#00000000").unwrap(), [0.0; 4]);
        assert_eq!(parse_color("rgba(255,0,0,0.5)").unwrap(), [1.0, 0.0, 0.0, 0.5]);
        assert!(parse_color("#ggg").is_err());
    }
}
