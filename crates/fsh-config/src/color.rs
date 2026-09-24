use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// An sRGB colour with alpha, written as `#rgb`, `#rgba`, `#rrggbb` or `#rrggbbaa`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    pub const fn with_alpha(self, a: u8) -> Self {
        Self { a, ..self }
    }
}

impl FromStr for Rgba {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let hex = s.trim().strip_prefix('#').ok_or_else(|| format!("colour `{s}` must start with `#`"))?;
        let bad = || format!("`{s}` is not a valid colour (use #rrggbb or #rrggbbaa)");
        let nibble = |c: u8| (c as char).to_digit(16).map(|d| d as u8).ok_or_else(bad);
        let b = hex.as_bytes();
        let (r, g, bl, a) = match b.len() {
            3 | 4 => {
                let x = |i: usize| nibble(b[i]).map(|v| v * 17);
                (x(0)?, x(1)?, x(2)?, if b.len() == 4 { x(3)? } else { 255 })
            }
            6 | 8 => {
                let x = |i: usize| Ok::<_, String>(nibble(b[i])? * 16 + nibble(b[i + 1])?);
                (x(0)?, x(2)?, x(4)?, if b.len() == 8 { x(6)? } else { 255 })
            }
            _ => return Err(bad()),
        };
        Ok(Self { r, g, b: bl, a })
    }
}

impl fmt::Display for Rgba {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02x}{:02x}{:02x}", self.r, self.g, self.b)?;
        if self.a != 255 {
            write!(f, "{:02x}", self.a)?;
        }
        Ok(())
    }
}

impl Serialize for Rgba {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Rgba {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_forms() {
        assert_eq!("#fff".parse::<Rgba>().unwrap(), Rgba::rgb(255, 255, 255));
        assert_eq!("#0f08".parse::<Rgba>().unwrap(), Rgba { r: 0, g: 255, b: 0, a: 0x88 });
        assert_eq!("#3daee9".parse::<Rgba>().unwrap(), Rgba::rgb(0x3d, 0xae, 0xe9));
        assert_eq!("#1f222780".parse::<Rgba>().unwrap(), Rgba::rgb(0x1f, 0x22, 0x27).with_alpha(0x80));
    }

    #[test]
    fn rejects_garbage() {
        for bad in ["fff", "#ff", "#12345", "#gggggg", "", "#"] {
            assert!(bad.parse::<Rgba>().is_err(), "{bad} should fail");
        }
    }

    #[test]
    fn displays_round_trip() {
        for s in ["#3daee9", "#1f222780"] {
            assert_eq!(s.parse::<Rgba>().unwrap().to_string(), s);
        }
    }
}
