use crate::wml::w_attr;
use crate::xml::{NodeId, XmlTree};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Colour {
    /// Black or white, whichever reads on the fill behind it.
    #[default]
    Auto,
    Rgb(Rgb),
}

/// The theme's colour scheme, in `w:themeColor` order: dark 1, light 1,
/// dark 2, light 2, accents 1–6, hyperlink, followed hyperlink.
#[derive(Debug, Clone, Default)]
pub(super) struct Palette(pub(super) [Option<Rgb>; 12]);

pub(super) const SCHEME_SLOTS: [&str; 12] = [
    "dk1", "lt1", "dk2", "lt2", "accent1", "accent2", "accent3", "accent4", "accent5", "accent6",
    "hlink", "folHlink",
];

/// Word's `w:highlight` names and colours.
const HIGHLIGHTS: [(&str, u32); 16] = [
    ("black", 0x000000),
    ("blue", 0x0000FF),
    ("cyan", 0x00FFFF),
    ("green", 0x00FF00),
    ("magenta", 0xFF00FF),
    ("red", 0xFF0000),
    ("yellow", 0xFFFF00),
    ("white", 0xFFFFFF),
    ("darkBlue", 0x000080),
    ("darkCyan", 0x008080),
    ("darkGreen", 0x008000),
    ("darkMagenta", 0x800080),
    ("darkRed", 0x800000),
    ("darkYellow", 0x808000),
    ("darkGray", 0x808080),
    ("lightGray", 0xC0C0C0),
];

impl std::fmt::Display for Rgb {
    /// `RRGGBB`, as WordprocessingML writes it.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }
}

impl std::fmt::Display for Colour {
    /// `auto` or `RRGGBB`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Colour::Auto => f.write_str("auto"),
            Colour::Rgb(rgb) => rgb.fmt(f),
        }
    }
}

impl Rgb {
    const WHITE: Rgb = Rgb::packed(0xFFFFFF);

    const fn packed(value: u32) -> Rgb {
        Rgb {
            r: (value >> 16) as u8,
            g: (value >> 8) as u8,
            b: value as u8,
        }
    }

    pub(super) fn hex(value: &str) -> Option<Rgb> {
        (value.len() == 6 && value.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| u32::from_str_radix(value, 16).ok())
            .flatten()
            .map(Rgb::packed)
    }

    /// `amount` of `self` over `under`.
    fn over(self, under: Rgb, amount: f64) -> Rgb {
        let mix =
            |a: u8, b: u8| (f64::from(a) * amount + f64::from(b) * (1.0 - amount)).round() as u8;
        Rgb {
            r: mix(self.r, under.r),
            g: mix(self.g, under.g),
            b: mix(self.b, under.b),
        }
    }

    /// ECMA-376 §17.3.2.6: in HSL, a tint moves luminance toward white
    /// (`L·t + 1 − t`), a shade toward black (`L·s`), `t` and `s` in 0–1.
    /// Given both, Word applies the tint only (MS-OI29500 §2.1.72).
    fn tinted(self, tint: Option<f64>, shade: Option<f64>) -> Rgb {
        if tint.is_none() && shade.is_none() {
            return self;
        }
        let (h, s, l) = self.hsl();
        let l = match (tint, shade) {
            (Some(tint), _) => l * tint + 1.0 - tint,
            (None, shade) => l * shade.unwrap_or(1.0),
        };
        Rgb::from_hsl(h, s, l.clamp(0.0, 1.0))
    }

    fn hsl(self) -> (f64, f64, f64) {
        let [r, g, b] = [self.r, self.g, self.b].map(|c| f64::from(c) / 255.0);
        let (max, min) = (r.max(g).max(b), r.min(g).min(b));
        let l = (max + min) / 2.0;
        let d = max - min;
        if d == 0.0 {
            return (0.0, 0.0, l);
        }
        let s = d / (1.0 - (2.0 * l - 1.0).abs());
        let h = if max == r {
            ((g - b) / d).rem_euclid(6.0)
        } else if max == g {
            (b - r) / d + 2.0
        } else {
            (r - g) / d + 4.0
        };
        (h / 6.0, s, l)
    }

    fn from_hsl(h: f64, s: f64, l: f64) -> Rgb {
        let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
        let h = h * 6.0;
        let x = c * (1.0 - (h.rem_euclid(2.0) - 1.0).abs());
        let (r, g, b) = match h as u8 {
            0 => (c, x, 0.0),
            1 => (x, c, 0.0),
            2 => (0.0, c, x),
            3 => (0.0, x, c),
            4 => (x, 0.0, c),
            _ => (c, 0.0, x),
        };
        let m = l - c / 2.0;
        // Truncating is within one unit of Word's cached values (rounding is
        // further off): 43 of 70 in the fixtures match, the rest are one off.
        let byte = |v: f64| ((v + m) * 255.0 + 1e-6).floor().clamp(0.0, 255.0) as u8;
        Rgb {
            r: byte(r),
            g: byte(g),
            b: byte(b),
        }
    }
}

impl Palette {
    fn theme(&self, name: &str) -> Option<Rgb> {
        let slot = match name {
            "dark1" | "text1" => 0,
            "light1" | "background1" => 1,
            "dark2" | "text2" => 2,
            "light2" | "background2" => 3,
            "accent1" => 4,
            "accent2" => 5,
            "accent3" => 6,
            "accent4" => 7,
            "accent5" => 8,
            "accent6" => 9,
            "hyperlink" => 10,
            "followedHyperlink" => 11,
            _ => return None,
        };
        self.0[slot]
    }

    /// The colour a property gives through attributes `[value, theme, tint,
    /// shade]`; a theme colour wins over the value Word caches beside it.
    pub(super) fn colour(&self, tree: &XmlTree, node: NodeId, names: [&str; 4]) -> Option<Colour> {
        let [value, theme, tint, shade] = names.map(|name| w_attr(tree, node, name));
        let byte = |v: Option<&str>| {
            v.and_then(|v| u8::from_str_radix(v, 16).ok())
                .map(|b| f64::from(b) / 255.0)
        };
        if let Some(rgb) = theme.and_then(|name| self.theme(name)) {
            return Some(Colour::Rgb(rgb.tinted(byte(tint), byte(shade))));
        }
        match value? {
            "auto" => Some(Colour::Auto),
            hex => Rgb::hex(hex).map(Colour::Rgb),
        }
    }

    /// `w:shd`'s fill as one colour: the pattern colour over the fill at
    /// the pattern's density. `None` is no fill.
    pub(super) fn shading(&self, tree: &XmlTree, shd: NodeId) -> Option<Rgb> {
        let fill = self.colour(
            tree,
            shd,
            ["fill", "themeFill", "themeFillTint", "themeFillShade"],
        );
        let pattern = self.colour(
            tree,
            shd,
            ["color", "themeColor", "themeTint", "themeShade"],
        );
        let density = match w_attr(tree, shd, "val") {
            None | Some("nil" | "clear") => 0.0,
            Some("solid") => 1.0,
            Some(pct) => pct
                .strip_prefix("pct")
                .and_then(|n| n.parse::<u8>().ok())
                .map_or(0.0, |n| (percent(n) / 100.0).clamp(0.0, 1.0)),
        };
        let solid = |c: Option<Colour>| match c {
            Some(Colour::Rgb(rgb)) => Some(rgb),
            _ => None,
        };
        let (fill, pattern) = (solid(fill), solid(pattern));
        if density == 0.0 {
            return fill;
        }
        // An automatic pattern colour is black.
        let ink = pattern.unwrap_or(Rgb::packed(0));
        Some(ink.over(fill.unwrap_or(Rgb::WHITE), density))
    }
}

/// `pct12`, `pct37`, `pct62` and `pct87` are 12.5, 37.5, 62.5 and 87.5 %.
fn percent(n: u8) -> f64 {
    match n {
        12 | 37 | 62 | 87 => f64::from(n) + 0.5,
        _ => f64::from(n),
    }
}

pub(super) fn highlight(name: &str) -> Option<Rgb> {
    HIGHLIGHTS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, v)| Rgb::packed(*v))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tint_and_shade_give_word_s_theme_variants() {
        let blue = Rgb::hex("4472C4").unwrap();
        let byte = |b: u8| Some(f64::from(b) / 255.0);
        // Word's Accent 1 lighter 80, 60 and 40 %, darker 25 and 50 %.
        for (tint, shade, want) in [
            (byte(0x33), None, "D9E2F3"),
            (byte(0x66), None, "B4C6E7"),
            (byte(0x99), None, "8EAADB"),
            (None, byte(0xBF), "2F5496"),
            (None, byte(0x80), "1F3864"),
            (None, None, "4472C4"),
        ] {
            assert_eq!(blue.tinted(tint, shade), Rgb::hex(want).unwrap(), "{want}");
        }
    }

    #[test]
    fn other_theme_variants_are_within_one_unit_of_word() {
        let byte = |b: u8| Some(f64::from(b) / 255.0);
        // Values Word cached in the fixtures (Office 2007 theme); MS-OI29500
        // §2.1.72 gives C0504D shade BF as 943634.
        for (base, tint, shade, word) in [
            ("4F81BD", None, byte(0xBF), "365F91"),
            ("C0504D", None, byte(0xBF), "943634"),
            ("F79646", byte(0x3F), None, "FDE4D0"),
        ] {
            let ours = Rgb::hex(base).unwrap().tinted(tint, shade);
            let word = Rgb::hex(word).unwrap();
            let off =
                [(ours.r, word.r), (ours.g, word.g), (ours.b, word.b)].map(|(a, b)| a.abs_diff(b));
            assert!(off.iter().all(|d| *d <= 1), "{base}: {ours} against {word}");
        }
    }

    #[test]
    fn given_tint_and_shade_word_applies_the_tint() {
        let blue = Rgb::hex("4472C4").unwrap();
        let byte = |b: u8| Some(f64::from(b) / 255.0);
        assert_eq!(
            blue.tinted(byte(0x99), byte(0xBF)),
            blue.tinted(byte(0x99), None)
        );
    }

    #[test]
    fn a_hex_colour_is_six_hex_digits() {
        assert_eq!(Rgb::hex("+12345"), None);
        assert_eq!(Rgb::hex("12345G"), None);
        assert_eq!(
            Rgb::hex("00ff7F"),
            Some(Rgb {
                r: 0,
                g: 255,
                b: 127
            })
        );
    }

    #[test]
    fn the_eighth_patterns_are_half_a_percent_more() {
        assert_eq!(percent(12), 12.5);
        assert_eq!(percent(87), 87.5);
        assert_eq!(percent(50), 50.0);
    }

    #[test]
    fn a_pattern_mixes_its_colour_over_the_fill() {
        let black = Rgb::packed(0);
        assert_eq!(black.over(Rgb::WHITE, 0.5), Rgb::hex("808080").unwrap());
        assert_eq!(black.over(Rgb::WHITE, 0.0), Rgb::WHITE);
    }
}
