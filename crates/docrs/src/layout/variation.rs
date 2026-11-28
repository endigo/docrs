//! Variable fonts: the `fvar` axes and named instances, and the one rule
//! that turns a run's weight and italic into axis values. With the `pdf`
//! feature, layout measures and the PDF writer embeds the same instance;
//! without it, layout measures a variable font's default instance.

#![cfg_attr(
    not(feature = "pdf"),
    allow(dead_code, reason = "only the pdf feature instances fonts")
)]

use super::fonts::be16;
use super::fonts::be32;

/// The weight, italic and size a run asks for; a static face ignores them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Style {
    pub weight: u16,
    pub italic: bool,
    /// The drawn size in points, for optical sizing; `None`: none.
    pub size: Option<f32>,
}

impl Eq for Style {}

impl std::hash::Hash for Style {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        (self.weight, self.italic, self.size.map(f32::to_bits)).hash(state);
    }
}

impl Style {
    /// A run's style: bold is weight 700, else 400, at the drawn size.
    pub(crate) fn of(bold: bool, italic: bool, size: Option<f64>) -> Style {
        Style {
            weight: if bold { 700 } else { 400 },
            italic,
            size: size.map(|size| size as f32),
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Style {
    pub const REGULAR: Style = Style {
        weight: 400,
        italic: false,
        size: None,
    };
}

/// A variation axis: tag, minimum, default and maximum.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Axis {
    pub tag: [u8; 4],
    pub min: f32,
    pub default: f32,
    pub max: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct NamedInstance {
    pub subfamily: u16,
    pub postscript: Option<u16>,
    /// One value per axis of [`Fvar::axes`].
    pub coordinates: Vec<f32>,
}

/// Axis values away from the default location, in axis order.
pub(crate) type Variation = Vec<([u8; 4], f32)>;

/// A font's `fvar`, without the axes whose range does not hold their default.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Fvar {
    pub axes: Vec<Axis>,
    pub instances: Vec<NamedInstance>,
}

impl Fvar {
    pub(crate) fn read(fvar: &[u8]) -> Fvar {
        let field = |at| be16(fvar, at).map(usize::from);
        let (Some(axes_at), Some(count), Some(axis_size), Some(instances), Some(instance_size)) =
            (field(4), field(8), field(10), field(12), field(14))
        else {
            return Fvar::default();
        };
        if axis_size < 20 {
            return Fvar::default();
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
            return Fvar::default();
        };
        let valid: Vec<bool> = axes
            .iter()
            .map(|axis| axis.min <= axis.default && axis.default <= axis.max)
            .collect();
        let start = axes_at + count * axis_size;
        let records_fit = instances
            .checked_mul(instance_size)
            .and_then(|length| start.checked_add(length))
            .is_some_and(|end| end <= fvar.len());
        let instances = if instance_size < 4 + 4 * count || !records_fit {
            0
        } else {
            instances
        };
        let instances = (0..instances)
            .map_while(|index| {
                let at = start + index * instance_size;
                let coordinates: Option<Vec<f32>> = (0..count)
                    .filter(|axis| valid[*axis])
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
        let axes = axes
            .into_iter()
            .zip(valid)
            .filter_map(|(axis, valid)| valid.then_some(axis))
            .collect();
        Fvar { axes, instances }
    }

    /// The instance `style` selects: `wght` set to the weight, `ital`
    /// (else `slnt`) to its italic end when the style is italic, and below
    /// weight 600 `opsz` to the size in 1/300 inch, rounded half up. Word
    /// for Mac (measured on Inter, Fraunces and Literata): regular runs are
    /// sized optically, bold runs take the Bold instance, `opsz` at its
    /// default, at every size.
    pub(crate) fn variation(&self, style: Style) -> Variation {
        let has_ital = self.axes.iter().any(|axis| &axis.tag == b"ital");
        let mut variation = Variation::new();
        for axis in &self.axes {
            let value = match &axis.tag {
                b"wght" => f32::from(style.weight).clamp(axis.min, axis.max),
                b"ital" if style.italic => axis.max,
                b"slnt" if style.italic && !has_ital => axis.min,
                b"opsz" if style.weight < 600 => match style.size {
                    Some(size) => (size * 300.0 / 72.0 + 0.5)
                        .floor()
                        .clamp(axis.min, axis.max),
                    None => continue,
                },
                _ => continue,
            };
            if value != axis.default {
                variation.push((axis.tag, value));
            }
        }
        variation
    }

    /// Every axis's value at `variation`, in axis order.
    pub(crate) fn coordinates(&self, variation: &Variation) -> Vec<f32> {
        self.axes
            .iter()
            .map(|axis| {
                variation
                    .iter()
                    .find(|(tag, _)| *tag == axis.tag)
                    .map_or(axis.default, |(_, value)| *value)
            })
            .collect()
    }
}

fn fixed(table: &[u8], at: usize) -> Option<f32> {
    be32(table, at).map(|v| v as i32 as f32 / 65536.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An `fvar` with the given axes and no named instances.
    fn fvar(axes: &[([u8; 4], i32, i32, i32)]) -> Vec<u8> {
        let mut out = Vec::new();
        for field in [
            1_u16,
            0,
            16,
            2,
            axes.len() as u16,
            20,
            0,
            4 + 4 * axes.len() as u16,
        ] {
            out.extend(field.to_be_bytes());
        }
        for (tag, min, default, max) in axes {
            out.extend(tag);
            for value in [min, default, max] {
                out.extend((value << 16).to_be_bytes());
            }
            out.extend([0, 0, 1, 0]);
        }
        out
    }

    fn style(weight: u16, italic: bool) -> Style {
        Style {
            weight,
            italic,
            size: None,
        }
    }

    #[test]
    fn the_weight_is_clamped_to_the_axis() {
        let fvar = Fvar::read(&fvar(&[(*b"wght", 100, 400, 900)]));
        assert_eq!(fvar.variation(style(700, false)), [(*b"wght", 700.0)]);
        assert_eq!(fvar.variation(style(1000, false)), [(*b"wght", 900.0)]);
        assert!(fvar.variation(Style::REGULAR).is_empty());
    }

    #[test]
    fn italic_takes_the_ital_axis_else_slnt() {
        let slanted = Fvar::read(&fvar(&[(*b"wght", 100, 400, 900), (*b"slnt", -10, 0, 0)]));
        assert_eq!(
            slanted.variation(style(700, true)),
            [(*b"wght", 700.0), (*b"slnt", -10.0)]
        );
        let both = Fvar::read(&fvar(&[(*b"slnt", -10, 0, 0), (*b"ital", 0, 0, 1)]));
        assert_eq!(both.variation(style(400, true)), [(*b"ital", 1.0)]);
    }

    #[test]
    fn an_axis_whose_range_misses_its_default_is_dropped() {
        let fvar = Fvar::read(&fvar(&[
            (*b"wght", 950, 400, 900),
            (*b"wdth", 75, 100, 50),
            (*b"slnt", -10, 0, 0),
        ]));
        assert_eq!(fvar.axes.len(), 1);
        assert_eq!(fvar.axes[0].tag, *b"slnt");
        assert!(fvar.variation(style(700, false)).is_empty());
    }

    #[test]
    fn regular_runs_are_sized_optically_and_bold_runs_are_not() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/fonts/inter/Inter[opsz,wght].ttf"
        );
        let font = std::fs::read(path).unwrap();
        let ttf = crate::layout::fonts::Ttf::face(&font, 0).unwrap();
        let inter = Fvar::read(ttf.table(b"fvar").unwrap());
        for (size, opsz) in [
            (4.0, 17.0),
            (6.5, 27.0),
            (8.0, 32.0),
            (11.0, 32.0),
            (24.0, 32.0),
        ] {
            let regular = inter.variation(Style::of(false, false, Some(size)));
            assert_eq!(regular, [(*b"opsz", opsz)], "{size} pt");
            let bold = inter.variation(Style::of(true, false, Some(size)));
            assert_eq!(bold, [(*b"wght", 700.0)], "{size} pt");
        }
        assert!(inter.variation(Style::REGULAR).is_empty());
        let fraunces = Fvar::read(&fvar(&[(*b"opsz", 9, 9, 144), (*b"wght", 100, 900, 900)]));
        for (size, opsz) in [
            (9.0, 38.0),
            (12.0, 50.0),
            (18.0, 75.0),
            (24.0, 100.0),
            (36.0, 144.0),
        ] {
            let regular = fraunces.variation(Style::of(false, false, Some(size)));
            assert_eq!(regular, [(*b"opsz", opsz), (*b"wght", 400.0)], "{size} pt");
        }
    }

    #[test]
    fn named_instances_must_fit_the_table() {
        let axes: Vec<_> = (0..64)
            .map(|n| ([b'a', b'x', b'0' + n / 10, b'0' + n % 10], 0, 0, 1))
            .collect();
        let mut table = fvar(&axes);
        for (at, value) in [(12, 0xFFFF_u16), (14, 0)] {
            table[at..at + 2].copy_from_slice(&value.to_be_bytes());
        }
        let read = Fvar::read(&table);
        assert_eq!(read.axes.len(), 64);
        assert!(read.instances.is_empty(), "zero-size records would repeat");
        table[14..16].copy_from_slice(&(4 + 4 * 64_u16).to_be_bytes());
        assert!(
            Fvar::read(&table).instances.is_empty(),
            "65,535 records overrun the table"
        );
        let record = [vec![0, 2, 0, 0], vec![0; 4 * 64]].concat();
        table[12..14].copy_from_slice(&2_u16.to_be_bytes());
        table.extend(record.repeat(2));
        let read = Fvar::read(&table);
        assert_eq!(read.instances.len(), 2);
        assert_eq!(read.instances[0].coordinates.len(), 64);
    }
}
