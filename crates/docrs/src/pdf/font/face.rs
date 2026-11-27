//! What the PDF needs to know about one source face: its permission to
//! embed, its descriptor metrics and its variation axes.

use crate::layout::fonts::{FontError, Ttf, be16, be32, licence, names};
use crate::{Error, Result};

/// The weight and italic a run asks for; a static face ignores both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Style {
    pub weight: u16,
    pub italic: bool,
}

impl Style {
    pub const REGULAR: Style = Style {
        weight: 400,
        italic: false,
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Outlines {
    TrueType,
    Cff,
    Cff2,
}

/// A variation axis: tag, minimum, default and maximum.
#[derive(Debug, Clone, Copy)]
struct Axis {
    tag: [u8; 4],
    min: f32,
    default: f32,
    max: f32,
}

#[derive(Debug, Clone)]
struct NamedInstance {
    subfamily: u16,
    postscript: Option<u16>,
    coordinates: Vec<f32>,
}

/// Axis values away from the default location, in axis order.
pub(super) type Variation = Vec<([u8; 4], f32)>;

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
    axes: Vec<Axis>,
    instances: Vec<NamedInstance>,
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
                "pdf_font_embedding_restricted",
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
        let (axes, instances) = ttf.table(b"fvar").map(variations).unwrap_or_default();
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
            axes,
            instances,
            names,
        })
    }

    /// The instance a style selects: `wght` set to the weight, and `ital`
    /// (else `slnt`) to its italic end when the style is italic.
    pub(super) fn instance(&self, style: Style) -> Instance {
        let has_ital = self.axes.iter().any(|axis| &axis.tag == b"ital");
        let mut variation = Variation::new();
        for axis in &self.axes {
            let value = match &axis.tag {
                b"wght" => f32::from(style.weight).clamp(axis.min, axis.max),
                b"ital" if style.italic => axis.max,
                b"slnt" if style.italic && !has_ital => axis.min,
                _ => continue,
            };
            if value != axis.default {
                variation.push((axis.tag, value));
            }
        }
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
    /// family prefix and each axis value away from the default.
    fn instance_name(&self, variation: &Variation) -> String {
        if variation.is_empty() {
            return self.postscript_name.clone();
        }
        let coordinates: Vec<f32> = self
            .axes
            .iter()
            .map(|axis| {
                variation
                    .iter()
                    .find(|(tag, _)| *tag == axis.tag)
                    .map_or(axis.default, |(_, value)| *value)
            })
            .collect();
        let prefix = name(&self.names, 25)
            .or_else(|| name(&self.names, 16))
            .or_else(|| name(&self.names, 1))
            .map(|prefix| postscript(&prefix))
            .unwrap_or_else(|| self.postscript_name.clone());
        let named = self
            .instances
            .iter()
            .find(|instance| instance.coordinates == coordinates);
        if let Some(instance) = named {
            if let Some(own) = instance.postscript.and_then(|id| name(&self.names, id)) {
                return postscript(&own);
            }
            if let Some(subfamily) = name(&self.names, instance.subfamily) {
                return format!("{prefix}-{}", postscript(&subfamily));
            }
        }
        let mut out = prefix;
        for (index, (tag, value)) in variation.iter().enumerate() {
            out.push(if index == 0 { '-' } else { '_' });
            out.push_str(&format!("{value}{}", String::from_utf8_lossy(tag)));
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

fn fixed(table: &[u8], at: usize) -> Option<f32> {
    be32(table, at).map(|v| v as i32 as f32 / 65536.0)
}

fn variations(fvar: &[u8]) -> (Vec<Axis>, Vec<NamedInstance>) {
    let field = |at| be16(fvar, at).map(usize::from);
    let (Some(axes_at), Some(count), Some(axis_size), Some(instances), Some(instance_size)) =
        (field(4), field(8), field(10), field(12), field(14))
    else {
        return Default::default();
    };
    if axis_size < 20 {
        return Default::default();
    }
    let axes: Option<Vec<Axis>> = (0..count)
        .map(|index| {
            let at = axes_at + index * axis_size;
            Some(Axis {
                tag: fvar.get(at..at + 4)?.try_into().ok()?,
                min: fixed(fvar, at + 4)?,
                default: fixed(fvar, at + 8)?,
                max: fixed(fvar, at + 12)?,
            })
        })
        .collect();
    let Some(axes) = axes else {
        return Default::default();
    };
    let start = axes_at + count * axis_size;
    let named = (0..instances)
        .map_while(|index| {
            let at = start + index * instance_size;
            let coordinates: Option<Vec<f32>> = (0..count)
                .map(|axis| fixed(fvar, at + 4 + 4 * axis))
                .collect();
            Some(NamedInstance {
                subfamily: be16(fvar, at)?,
                postscript: (instance_size >= count * 4 + 6)
                    .then(|| be16(fvar, at + 4 + 4 * count))
                    .flatten()
                    .filter(|id| *id != 0xFFFF),
                coordinates: coordinates?,
            })
        })
        .collect();
    (axes, named)
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
        Style { weight, italic }
    }

    #[test]
    fn instances_are_named_as_adobe_tn_5902_names_them() {
        let inter = inter();
        let name = |weight| inter.instance(style(weight, false)).postscript_name;
        assert_eq!(name(400), "Inter-Regular");
        assert_eq!(name(700), "Inter-Bold");
        assert_eq!(name(650), "Inter-650wght");
        assert_eq!(name(1000), "Inter-Black");
    }

    #[test]
    fn the_weight_axis_sets_the_descriptor_weight() {
        let inter = inter();
        let bold = inter.instance(style(700, false));
        assert_eq!(bold.variation, [(*b"wght", 700.0)]);
        assert_eq!((bold.weight, bold.italic), (700, false));
        let regular = inter.instance(style(400, true));
        assert!(
            regular.variation.is_empty(),
            "Inter has no ital or slnt axis"
        );
        assert_eq!(regular, inter.instance(Style::REGULAR));
    }

    #[test]
    fn italic_takes_the_ital_axis_else_slnt() {
        let axis = |tag: &[u8; 4], min, default, max| Axis {
            tag: *tag,
            min,
            default,
            max,
        };
        let wght = axis(b"wght", 100.0, 400.0, 900.0);
        let slnt = axis(b"slnt", -10.0, 0.0, 0.0);
        let ital = axis(b"ital", 0.0, 0.0, 1.0);
        let face = |axes| FaceInfo {
            axes,
            instances: Vec::new(),
            ..inter()
        };
        let slanted = face(vec![wght, slnt]).instance(style(700, true));
        assert_eq!(slanted.variation, [(*b"wght", 700.0), (*b"slnt", -10.0)]);
        assert_eq!((slanted.italic_angle, slanted.italic), (-10.0, true));
        assert_eq!(slanted.postscript_name, "Inter-700wght_-10slnt");
        let italic = face(vec![wght, slnt, ital]).instance(style(400, true));
        assert_eq!(italic.variation, [(*b"ital", 1.0)]);
        assert_eq!((italic.italic_angle, italic.italic), (0.0, true));
    }
}
