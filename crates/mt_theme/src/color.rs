//! Colour parsing, OKLab/OKLCH conversion and WCAG contrast. Hand-rolled: no colour crate.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

    /// `#rgb`, `#rrggbb` or `#rrggbbaa`.
    pub fn parse(s: &str) -> Result<Self, String> {
        let h = s
            .strip_prefix('#')
            .ok_or_else(|| format!("colour \"{s}\" must start with '#'"))?;
        if !h.is_ascii() {
            return Err(format!("invalid colour \"{s}\""));
        }
        let byte = |i: usize| {
            u8::from_str_radix(&h[i..i + 2], 16).map_err(|_| format!("invalid colour \"{s}\""))
        };
        match h.len() {
            3 => {
                let n = |i: usize| {
                    u8::from_str_radix(&h[i..i + 1], 16)
                        .map(|v| v * 17)
                        .map_err(|_| format!("invalid colour \"{s}\""))
                };
                Ok(Self::rgb(n(0)?, n(1)?, n(2)?))
            }
            6 => Ok(Self::rgb(byte(0)?, byte(2)?, byte(4)?)),
            8 => Ok(Self {
                r: byte(0)?,
                g: byte(2)?,
                b: byte(4)?,
                a: byte(6)?,
            }),
            _ => Err(format!("invalid colour \"{s}\"")),
        }
    }

    /// `0xRRGGBB`.
    pub fn to_u32(self) -> u32 {
        (u32::from(self.r) << 16) | (u32::from(self.g) << 8) | u32::from(self.b)
    }

    pub fn alpha(self) -> f32 {
        f32::from(self.a) / 255.0
    }

    /// WCAG relative luminance.
    pub fn luminance(self) -> f32 {
        let l = |c: u8| srgb_to_linear(f32::from(c) / 255.0);
        0.2126 * l(self.r) + 0.7152 * l(self.g) + 0.0722 * l(self.b)
    }

    /// WCAG contrast ratio, 1.0..=21.0.
    pub fn contrast(self, other: Self) -> f32 {
        let (a, b) = (self.luminance(), other.luminance());
        let (hi, lo) = if a > b { (a, b) } else { (b, a) };
        (hi + 0.05) / (lo + 0.05)
    }

    /// Linear mix in sRGB: `t = 0` is `self`, `t = 1` is `other`.
    pub fn mix(self, other: Self, t: f32) -> Self {
        let m = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
        Self::rgb(m(self.r, other.r), m(self.g, other.g), m(self.b, other.b))
    }

    pub fn to_oklch(self) -> Oklch {
        let lin = |c: u8| srgb_to_linear(f32::from(c) / 255.0);
        let (r, g, b) = (lin(self.r), lin(self.g), lin(self.b));
        let l = (0.412_221_47 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
        let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
        let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
        let lab_l = 0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s;
        let a = 1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s;
        let bb = 0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s;
        Oklch {
            l: lab_l,
            c: a.hypot(bb),
            h: bb.atan2(a),
        }
    }
}

impl fmt::Display for Rgba {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.a == 255 {
            write!(f, "#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
        } else {
            write!(
                f,
                "#{:02x}{:02x}{:02x}{:02x}",
                self.r, self.g, self.b, self.a
            )
        }
    }
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// OKLCH: lightness 0..1, chroma, hue in radians.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Oklch {
    pub l: f32,
    pub c: f32,
    pub h: f32,
}

impl Oklch {
    /// Back to sRGB. Out-of-gamut colours are brought in by reducing chroma.
    pub fn to_rgba(self) -> Rgba {
        let mut c = self.c;
        for _ in 0..24 {
            if let Some(rgb) = self.with_chroma(c) {
                return rgb;
            }
            c *= 0.9;
        }
        let g = (linear_to_srgb(self.l.powi(3).clamp(0.0, 1.0)) * 255.0).round() as u8;
        Rgba::rgb(g, g, g)
    }

    fn with_chroma(self, c: f32) -> Option<Rgba> {
        let (a, b) = (c * self.h.cos(), c * self.h.sin());
        let l = (self.l + 0.396_337_78 * a + 0.215_803_76 * b).powi(3);
        let m = (self.l - 0.105_561_346 * a - 0.063_854_17 * b).powi(3);
        let s = (self.l - 0.089_484_18 * a - 1.291_485_5 * b).powi(3);
        let r = 4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s;
        let g = -1.268_438 * l + 2.609_757_4 * m - 0.341_319_4 * s;
        let bl = -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s;
        const EPS: f32 = 0.001;
        if [r, g, bl].iter().any(|v| *v < -EPS || *v > 1.0 + EPS) {
            return None;
        }
        let q = |v: f32| (linear_to_srgb(v.clamp(0.0, 1.0)) * 255.0).round() as u8;
        Some(Rgba::rgb(q(r), q(g), q(bl)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_parsing() {
        assert_eq!(Rgba::parse("#fff").unwrap(), Rgba::rgb(255, 255, 255));
        assert_eq!(Rgba::parse("#282c33").unwrap().to_u32(), 0x282c33);
        assert_eq!(Rgba::parse("#74ade840").unwrap().a, 0x40);
        assert!(Rgba::parse("74ade8").is_err());
        assert!(Rgba::parse("#12345").is_err());
        assert!(Rgba::parse("#gggggg").is_err());
    }

    #[test]
    fn contrast_extremes() {
        let w = Rgba::rgb(255, 255, 255);
        let k = Rgba::rgb(0, 0, 0);
        assert!((w.contrast(k) - 21.0).abs() < 0.01);
        assert!((w.contrast(w) - 1.0).abs() < 1e-4);
    }

    #[test]
    fn oklch_round_trip() {
        for hex in ["#282c33", "#74ade8", "#fafafa", "#d07277", "#000000"] {
            let c = Rgba::parse(hex).unwrap();
            let back = c.to_oklch().to_rgba();
            for (a, b) in [(c.r, back.r), (c.g, back.g), (c.b, back.b)] {
                assert!(a.abs_diff(b) <= 1, "{hex}: {c} vs {back}");
            }
        }
    }

    #[test]
    fn white_has_lightness_one() {
        let l = Rgba::rgb(255, 255, 255).to_oklch().l;
        assert!((l - 1.0).abs() < 0.01);
    }
}
