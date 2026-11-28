//! What the PDF needs to know about one source face: its permission to
//! embed, its descriptor metrics and its variation axes.

use crate::layout::fonts::{FontError, Ttf, be16, be32, licence, names};
use crate::layout::variation::{Fvar, Style, Variation};
use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Outlines {
    TrueType,
    Cff,
    Cff2,
}

#[derive(Debug, Clone)]
pub(super) struct FaceInfo {
    /// Family and style, for messages.
    pub label: String,
    pub outlines: Outlines,
    pub subsettable: bool,
    pub glyphs: u16,
    pub units_per_em: f64,
    pub bbox: [f64; 4],
    pub ascent: f64,
    pub descent: f64,
    pub cap_height: f64,
    italic_angle: f64,
    weight: u16,
    italic: bool,
    pub fixed_pitch: bool,
    pub serif: bool,
    postscript_name: String,
    fvar: Fvar,
    names: Vec<u8>,
}

/// The descriptor values that depend on the instance.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Instance {
    pub variation: Variation,
    pub postscript_name: String,
    pub italic_angle: f64,
    pub weight: u16,
    pub italic: bool,
}

impl FaceInfo {
    pub(super) fn read(data: &[u8], index: u32) -> Result<FaceInfo> {
        let ttf = Ttf::face(data, index).map_err(FontError::into_error)?;
        let malformed = |table| FontError::Malformed(table).into_error();
        let names = ttf.table(b"name").unwrap_or_default().to_vec();
        let family = name(&names, 16).or_else(|| name(&names, 1));
        let subfamily = name(&names, 17).or_else(|| name(&names, 2));
        let label = match (family, subfamily) {
            (Some(family), Some(subfamily)) => format!("{family} {subfamily}"),
            (family, _) => family.unwrap_or_else(|| "a font without a name".into()),
        };
        let outlines = if ttf.table(b"glyf").is_some() {
            Outlines::TrueType
        } else if ttf.table(b"CFF ").is_some() {
            Outlines::Cff
        } else if ttf.table(b"CFF2").is_some() {
            Outlines::Cff2
        } else {
            return Err(Error::invalid_argument(
                "pdf_font_no_outlines",
                format!("{label} has no outlines to embed"),
            ));
        };
        let (embeddable, subsettable) = licence(&ttf);
        if !embeddable {
            return Err(Error::invalid_argument(
                "pdf_font_embedding_restricted",
                format!("the licence of {label} does not allow embedding it in a PDF"),
            ));
        }
        if !subsettable && ttf.table(b"CFF ").is_some_and(cid_keyed) {
            return Err(Error::invalid_argument(
                "pdf_font_embedding_unsupported",
                format!(
                    "the licence of {label} allows embedding it only whole, and a whole CID-keyed CFF font is not supported"
                ),
            ));
        }
        let head = ttf.table(b"head").ok_or_else(|| malformed("head"))?;
        let hhea = ttf.table(b"hhea").ok_or_else(|| malformed("hhea"))?;
        let post = ttf.table(b"post").unwrap_or_default();
        let maxp = ttf.table(b"maxp").ok_or_else(|| malformed("maxp"))?;
        let units_per_em = be16(head, 18)
            .filter(|units| (16..=16384).contains(units))
            .ok_or_else(|| malformed("head"))?;
        let signed = |table: &[u8], at| be16(table, at).map_or(0.0, |v| f64::from(v as i16));
        let ascent = signed(hhea, 4);
        let os2 = ttf.table(b"OS/2").unwrap_or_default();
        let mac_style = be16(head, 44).unwrap_or(0);
        let fs_selection = be16(os2, 62).unwrap_or(0);
        let postscript_name = name(&names, 6)
            .map(|name| postscript(&name))
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| postscript(&label));
        Ok(FaceInfo {
            outlines,
            subsettable,
            glyphs: be16(maxp, 4)
                .filter(|n| *n > 0)
                .ok_or_else(|| malformed("maxp"))?,
            units_per_em: f64::from(units_per_em),
            bbox: [36, 38, 40, 42].map(|at| signed(head, at)),
            ascent,
            descent: signed(hhea, 6),
            cap_height: be16(os2, 0)
                .filter(|version| *version >= 2)
                .map_or(ascent, |_| signed(os2, 88)),
            italic_angle: be32(post, 4).map_or(0.0, |v| f64::from(v as i32) / 65536.0),
            weight: be16(os2, 4)
                .filter(|w| (1..=1000).contains(w))
                .unwrap_or(400),
            italic: mac_style & 2 != 0 || fs_selection & 1 != 0,
            fixed_pitch: be32(post, 12).is_some_and(|v| v != 0),
            serif: matches!(be16(os2, 30).map(|class| class >> 8), Some(1..=5 | 7)),
            postscript_name,
            label,
            fvar: ttf.table(b"fvar").map(Fvar::read).unwrap_or_default(),
            names,
        })
    }

    /// The instance `style` selects, by [`Fvar::variation`]; `None`: the
    /// default instance.
    pub(super) fn instance(&self, style: Option<Style>) -> Instance {
        let variation = style.map_or_else(Variation::new, |style| self.fvar.variation(style));
        let value = |tag: &[u8; 4]| {
            variation
                .iter()
                .find(|(t, _)| t == tag)
                .map(|(_, value)| *value)
        };
        let slant = value(b"slnt");
        Instance {
            postscript_name: self.instance_name(&variation),
            italic_angle: slant.map_or(self.italic_angle, f64::from),
            weight: value(b"wght").map_or(self.weight, |w| w.round() as u16),
            italic: self.italic || slant.is_some() || value(b"ital").is_some_and(|v| v > 0.0),
            variation,
        }
    }

    /// Adobe Technical Note #5902: a named instance's own name, else the
    /// family prefix (letters and digits) with the instance's subfamily
    /// after a hyphen, else with each axis value after an underscore.
    fn instance_name(&self, variation: &Variation) -> String {
        if variation.is_empty() {
            return self.postscript_name.clone();
        }
        let prefix: String = name(&self.names, 25)
            .or_else(|| name(&self.names, 16))
            .or_else(|| name(&self.names, 1))
            .unwrap_or_else(|| self.postscript_name.clone())
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .collect();
        let coordinates = self.fvar.coordinates(variation);
        let named = self
            .fvar
            .instances
            .iter()
            .find(|instance| instance.coordinates == coordinates);
        if let Some(instance) = named {
            if let Some(own) = instance.postscript.and_then(|id| name(&self.names, id)) {
                return postscript(&own);
            }
            if let Some(subfamily) = name(&self.names, instance.subfamily) {
                return postscript(&format!("{prefix}-{subfamily}"));
            }
        }
        let mut out = prefix;
        for (tag, value) in variation {
            out.push_str(&format!("_{value}{}", String::from_utf8_lossy(tag)));
        }
        postscript(&out)
    }
}

fn name(table: &[u8], id: u16) -> Option<String> {
    let [name] = names(table, [id]);
    name
}

/// A PostScript name: printable ASCII without delimiters, at most 63 bytes.
fn postscript(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_ascii_graphic() && !"[](){}<>/%".contains(*c))
        .take(63)
        .collect()
}

/// A CFF table whose Top DICT has a `ROS` operator (12 30): its charset
/// maps glyph ids to CIDs that need not equal them.
fn cid_keyed(cff: &[u8]) -> bool {
    let top_dict = cff.get(2).and_then(|size| {
        let (_, top_dicts) = index_entry(cff, usize::from(*size))?;
        index_entry(cff, top_dicts)
    });
    top_dict.is_some_and(|(dict, _)| has_ros(dict).unwrap_or(false))
}

fn has_ros(dict: &[u8]) -> Option<bool> {
    let mut at = 0;
    while let Some(&byte) = dict.get(at) {
        at += match byte {
            12 if dict.get(at + 1) == Some(&30) => return Some(true),
            12 | 28 | 247..=254 => 2 + usize::from(byte == 28),
            29 => 5,
            30 => {
                let end = dict[at + 1..]
                    .iter()
                    .position(|b| b & 0x0F == 0x0F || b >> 4 == 0x0F)?;
                end + 2
            }
            _ => 1,
        };
    }
    Some(false)
}

/// The first entry of the CFF INDEX at `at`, and where the INDEX ends.
fn index_entry(cff: &[u8], at: usize) -> Option<(&[u8], usize)> {
    let count = usize::from(be16(cff, at)?);
    if count == 0 {
        return Some((&[], at + 2));
    }
    let size = usize::from(*cff.get(at + 2)?);
    if !(1..=4).contains(&size) {
        return None;
    }
    let offset = |index: usize| -> Option<usize> {
        let start = at + 3 + index * size;
        let bytes = cff.get(start..start + size)?;
        Some(bytes.iter().fold(0, |sum, b| sum << 8 | usize::from(*b)))
    };
    let data = at + 2 + (count + 1) * size;
    let (first, second, last) = (offset(0)?, offset(1)?, offset(count)?);
    let entry = cff.get(data + first..data + second)?;
    Some((entry, data + last))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inter() -> FaceInfo {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/fonts/inter/Inter[opsz,wght].ttf"
        );
        FaceInfo::read(&std::fs::read(path).unwrap(), 0).unwrap()
    }

    fn style(weight: u16, italic: bool) -> Style {
        Style {
            weight,
            italic,
            size: None,
        }
    }

    #[test]
    fn instances_are_named_as_adobe_tn_5902_names_them() {
        let inter = inter();
        let name = |weight| inter.instance(Some(style(weight, false))).postscript_name;
        assert_eq!(name(400), "Inter-Regular");
        assert_eq!(name(700), "Inter-Bold");
        assert_eq!(name(650), "Inter_650wght");
        assert_eq!(name(1000), "Inter-Black");
    }

    #[test]
    fn the_weight_axis_sets_the_descriptor_weight() {
        let inter = inter();
        let bold = inter.instance(Some(style(700, false)));
        assert_eq!(bold.variation, [(*b"wght", 700.0)]);
        assert_eq!((bold.weight, bold.italic), (700, false));
        let regular = inter.instance(Some(style(400, true)));
        assert!(
            regular.variation.is_empty(),
            "Inter has no ital or slnt axis"
        );
        assert_eq!(regular, inter.instance(Some(Style::REGULAR)));
    }

    #[test]
    fn a_slant_sets_the_italic_angle() {
        let mut face = inter();
        face.fvar.axes.push(crate::layout::variation::Axis {
            tag: *b"slnt",
            min: -10.0,
            default: 0.0,
            max: 0.0,
        });
        face.fvar.instances.clear();
        let slanted = face.instance(Some(style(700, true)));
        assert_eq!((slanted.italic_angle, slanted.italic), (-10.0, true));
        assert_eq!(slanted.postscript_name, "Inter_700wght_-10slnt");
    }

    /// A CFF table: header, a one-name INDEX, then a Top DICT INDEX.
    fn cff(top_dict: &[u8]) -> Vec<u8> {
        let mut out = vec![1, 0, 4, 1, 0, 1, 1, 1, 2, b'A'];
        out.extend([0, 1, 1, 1, 1 + top_dict.len() as u8]);
        out.extend(top_dict);
        out
    }

    #[test]
    fn a_ros_operator_makes_a_cff_font_cid_keyed() {
        let ros = [139, 140, 139, 12, 30];
        assert!(cid_keyed(&cff(&ros)));
        let real_then_ros = [30, 0x1A, 0x2F, 139, 139, 139, 12, 30];
        assert!(cid_keyed(&cff(&real_then_ros)));
        let name_keyed = [28, 0, 5, 15, 139, 12, 7];
        assert!(!cid_keyed(&cff(&name_keyed)));
        assert!(!cid_keyed(&cff(&[30, 0x11])));
        assert!(!cid_keyed(&[1, 0]));
    }
}
