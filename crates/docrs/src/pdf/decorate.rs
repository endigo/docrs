//! What the painter draws around a line's text: highlight and shading
//! behind it, underlines and strikethrough over it.

use std::sync::Arc;

use super::writer::write_number;
use crate::Result;
use crate::layout::fonts::{Face, Stroke};
use crate::layout::model::{Appearance, Colour, Rgb, Strike, UnderlineStyle};

/// A piece of a line as decorations see it: page x, from the left edge.
pub(super) struct Span<'m> {
    pub look: &'m Appearance,
    pub face: Arc<Face>,
    pub from: f64,
    pub to: f64,
    /// A space or tab: a words-only underline leaves it out.
    pub blank: bool,
    /// An ordinary space after the line's last other piece: highlighted,
    /// not underlined or struck.
    pub trailing: bool,
}

/// Whether text in `look` has a fill or a line to draw.
pub(super) fn decorated(look: &Appearance) -> bool {
    look.highlight.is_some()
        || look.shade.is_some()
        || look.underline.is_some()
        || look.strike != Strike::None
}

/// The colour `colour` draws in, for text shaded `look`'s shading, else
/// the paragraph's. Word for Mac draws `auto` white on shading whose
/// BT.601 luma is below 75/255 (4A4A4A and C00000 white, 4C4C4C and
/// FF0000 black), else black; a highlight never changes it.
pub(super) fn ink(colour: Colour, look: &Appearance, paragraph: Option<Rgb>) -> [u8; 3] {
    match colour {
        Colour::Rgb(rgb) => [rgb.r, rgb.g, rgb.b],
        Colour::Auto if look.shade.or(paragraph).is_some_and(dark) => [255, 255, 255],
        _ => [0, 0, 0],
    }
}

fn dark(rgb: Rgb) -> bool {
    let luma = 0.299 * f64::from(rgb.r) + 0.587 * f64::from(rgb.g) + 0.114 * f64::from(rgb.b);
    luma < 75.0
}

/// Highlight and shading rectangles from the line's `top` to its bottom,
/// `height` below; `page` is the page height. Word fills the text's ascent
/// to descent, which is the same box under single spacing only.
pub(super) fn backgrounds(
    out: &mut Vec<u8>,
    spans: &[Span],
    page: f64,
    top: f64,
    height: f64,
) -> Result<()> {
    let rects = merged(spans, |span| span.look.highlight.or(span.look.shade));
    if rects.is_empty() {
        return Ok(());
    }
    out.extend_from_slice(b"q\n");
    for (rgb, _, from, to) in rects {
        colour(out, [rgb.r, rgb.g, rgb.b])?;
        out.extend_from_slice(b"rg ");
        numbers(out, [from, page - top - height, to - from, height])?;
        out.extend_from_slice(b"re f\n");
    }
    out.extend_from_slice(b"Q\n");
    Ok(())
}

/// One straight or wavy line over the text.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Mark {
    rgb: [u8; 3],
    from: f64,
    to: f64,
    /// Its centre, in page y.
    y: f64,
    width: f64,
    pattern: Pattern,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Pattern {
    Solid,
    /// Dash and gap lengths in points.
    Dashed(&'static [f64]),
    /// From peak to trough, and the band about its centre it is clipped
    /// to, in points.
    Wave {
        height: f64,
        clip: f64,
    },
}

// Word for Mac 16.115's underline shapes, measured on Inter 11 pt and
// Times New Roman 11 and 16 pt. Single, dotted and dashed lines take the
// face's `post` line; double, thick, heavy and wavy ones start at a top
// placed by the size and take widths from the face's thickness. Lengths
// are fixed in points, whatever the size or face.

/// Where double, thick, heavy and wavy lines start below the baseline, in
/// ems (0.72 pt at 11 pt, 0.96 at 16).
const SHAPE_TOP: f64 = 0.065;
/// Double: two lines half the face's thickness, this far apart.
const DOUBLE_GAP: f64 = 0.96;
/// Thick and heavy lines are this many face thicknesses wide.
const HEAVY: f64 = 2.0;
const DOTTED: [f64; 2] = [1.44, 1.44];
const DASH: [f64; 2] = [3.84, 1.92];
const DASH_LONG: [f64; 2] = [7.92, 3.84];
const DOT_DASH: [f64; 4] = [2.88, 0.96, 0.96, 0.96];
const DOT_DOT_DASH: [f64; 6] = [2.16, 0.72, 0.72, 0.72, 0.72, 0.72];
/// A wave: from peak to trough the face's thickness, its top this far
/// below the shapes' top; stroked 0.24 pt (heavy 0.96), 2.4 pt long,
/// clipped to a 2 pt band. A double wave is two plain ones 0.48 pt apart,
/// each clipped to 1 pt. Word's bands were measured at 11 pt; a band is
/// never narrower than its wave and stroke, so larger waves are not cut.
const WAVE_DROP: f64 = 0.12;
const WAVE_WIDTH: f64 = 0.24;
const WAVE_HEAVY_WIDTH: f64 = 0.96;
const WAVE_PERIOD: f64 = 2.4;
const WAVE_CLIP: f64 = 2.0;
const WAVE_DOUBLE_GAP: f64 = 0.48;
const WAVE_DOUBLE_CLIP: f64 = 1.0;

/// The underlines and strikethrough of a line whose baseline is at page y
/// `baseline`, over a paragraph shaded `paragraph`.
pub(super) fn decorations(
    out: &mut Vec<u8>,
    spans: &[Span],
    baseline: f64,
    paragraph: Option<Rgb>,
) -> Result<()> {
    let mut marks = underlines(spans, baseline, paragraph);
    marks.extend(strikes(spans, baseline, paragraph));
    if marks.is_empty() {
        return Ok(());
    }
    out.extend_from_slice(b"q\n");
    for mark in marks {
        draw(out, &mark)?;
    }
    out.extend_from_slice(b"Q\n");
    Ok(())
}

/// One underline under each stretch of underlined text, as Word draws
/// it, sized by its largest text.
fn underlines(spans: &[Span], baseline: f64, paragraph: Option<Rgb>) -> Vec<Mark> {
    let mut marks = Vec::new();
    let underlined = |span: &Span| span.look.underline.is_some() && !span.trailing;
    let mut rest = spans;
    while let Some(start) = rest.iter().position(underlined) {
        let end = rest[start..]
            .iter()
            .position(|span| !underlined(span))
            .map_or(rest.len(), |len| start + len);
        let stretch = &rest[start..end];
        rest = &rest[end..];
        let Some((size, line)) = stretch_line(stretch) else {
            continue;
        };
        let parts = merged(stretch, |span| {
            let underline = span.look.underline?;
            if underline.style == UnderlineStyle::Words && span.blank {
                return None;
            }
            let text = ink(span.look.colour, span.look, paragraph);
            Some((underline.style, ink_or(underline.colour, text)))
        });
        for ((style, rgb), _, from, to) in parts {
            let mark = Mark {
                rgb,
                from,
                to,
                y: baseline + line.offset,
                width: line.thickness,
                pattern: Pattern::Solid,
            };
            underline(&mut marks, style, mark, baseline - SHAPE_TOP * size);
        }
    }
    marks
}

/// Word's underline under a stretch mixing sizes is at 0.88 of the largest
/// run's position and 0.86 of its thickness (fitted to Word for Mac on
/// Inter, 8–24 pt: within 0.25 pt for two sizes; Word's own rule is not
/// known). Raised and lowered runs, super- and subscript included, are
/// underlined with the text on the baseline and do not count.
const MIXED_POSITION: f64 = 0.88;
const MIXED_THICKNESS: f64 = 0.86;

/// The size of a stretch's largest text, and the underline in points it
/// gets: that text's face's own, or Word's under mixed sizes.
fn stretch_line(stretch: &[Span]) -> Option<(f64, Stroke)> {
    let level: Vec<&Span> = stretch
        .iter()
        .filter(|span| span.look.rise == 0.0)
        .collect();
    let counted = if level.is_empty() {
        stretch.iter().collect()
    } else {
        level
    };
    let largest = counted
        .iter()
        .max_by(|a, b| a.look.size.total_cmp(&b.look.size))?;
    let size = largest.look.size;
    let line = scaled(largest.face.underline(), size);
    if counted.iter().all(|span| span.look.size == size) {
        return Some((size, line));
    }
    let top = (line.offset + line.thickness / 2.0) * MIXED_POSITION;
    let thickness = line.thickness * MIXED_THICKNESS;
    Some((
        size,
        Stroke {
            offset: top - thickness / 2.0,
            thickness,
        },
    ))
}

/// `colour`, or `text`'s for `auto`.
fn ink_or(colour: Colour, text: [u8; 3]) -> [u8; 3] {
    match colour {
        Colour::Rgb(rgb) => [rgb.r, rgb.g, rgb.b],
        Colour::Auto => text,
    }
}

/// `plain`, a single underline at the face's line, drawn in `style`;
/// `top` is the page y where the shapes Word places by size start.
fn underline(marks: &mut Vec<Mark>, style: UnderlineStyle, plain: Mark, top: f64) {
    use UnderlineStyle as U;
    let t = plain.width;
    let heavy = Mark {
        y: top - HEAVY * t / 2.0,
        width: HEAVY * t,
        ..plain
    };
    let dashed = |line: Mark, lengths: &'static [f64]| Mark {
        pattern: Pattern::Dashed(lengths),
        ..line
    };
    let centre = top - WAVE_DROP - t / 2.0;
    let wave = |y: f64, width: f64, clip: f64| Mark {
        y,
        width,
        pattern: Pattern::Wave {
            height: t,
            clip: clip.max(t + width),
        },
        ..plain
    };
    match style {
        U::Single | U::Words => marks.push(plain),
        U::Thick => marks.push(heavy),
        U::Double => marks.extend([0.0, t / 2.0 + DOUBLE_GAP].map(|below| Mark {
            y: top - below - t / 4.0,
            width: t / 2.0,
            ..plain
        })),
        U::Dotted => marks.push(dashed(plain, &DOTTED)),
        U::DottedHeavy => marks.push(dashed(heavy, &DOTTED)),
        U::Dash => marks.push(dashed(plain, &DASH)),
        U::DashedHeavy => marks.push(dashed(heavy, &DASH)),
        U::DashLong => marks.push(dashed(plain, &DASH_LONG)),
        U::DashLongHeavy => marks.push(dashed(heavy, &DASH_LONG)),
        U::DotDash => marks.push(dashed(plain, &DOT_DASH)),
        U::DashDotHeavy => marks.push(dashed(heavy, &DOT_DASH)),
        U::DotDotDash => marks.push(dashed(plain, &DOT_DOT_DASH)),
        U::DashDotDotHeavy => marks.push(dashed(heavy, &DOT_DOT_DASH)),
        U::Wave => marks.push(wave(centre, WAVE_WIDTH, WAVE_CLIP)),
        U::WavyHeavy => marks.push(wave(centre, WAVE_HEAVY_WIDTH, WAVE_CLIP)),
        U::WavyDouble => marks.extend([0.5, -0.5].map(|side| {
            wave(
                centre + side * WAVE_DOUBLE_GAP,
                WAVE_WIDTH,
                WAVE_DOUBLE_CLIP,
            )
        })),
    }
}

/// Strikethrough at each run's own line, raised with the run; a double
/// one is two lines a thickness apart on either side of it.
fn strikes(spans: &[Span], baseline: f64, paragraph: Option<Rgb>) -> Vec<Mark> {
    let parts = merged(spans, |span| {
        let look = span.look;
        (look.strike != Strike::None && !span.trailing).then(|| {
            let rgb = ink(look.colour, look, paragraph);
            (
                look.strike,
                Arc::as_ptr(&span.face),
                look.size.to_bits(),
                look.rise.to_bits(),
                rgb,
            )
        })
    });
    let mut marks = Vec::new();
    for ((strike, _, _, _, rgb), first, from, to) in parts {
        let line = scaled(first.face.strikeout(), first.look.size);
        let y = baseline + first.look.rise + line.offset;
        let mark = Mark {
            rgb,
            from,
            to,
            y,
            width: line.thickness,
            pattern: Pattern::Solid,
        };
        match strike {
            Strike::Double => {
                marks.push(Mark {
                    y: y + line.thickness,
                    ..mark
                });
                marks.push(Mark {
                    y: y - line.thickness,
                    ..mark
                });
            }
            _ => marks.push(mark),
        }
    }
    marks
}

fn scaled(stroke: Stroke, size: f64) -> Stroke {
    Stroke {
        offset: stroke.offset * size,
        thickness: stroke.thickness * size,
    }
}

/// Adjacent spans with the same `key`, joined: the key, the first span,
/// and the x they cover. Spans whose key is `None` are left out.
fn merged<'s, 'm, K: PartialEq>(
    spans: &'s [Span<'m>],
    key: impl Fn(&Span) -> Option<K>,
) -> Vec<(K, &'s Span<'m>, f64, f64)> {
    let mut out: Vec<(K, &Span, f64, f64)> = Vec::new();
    let mut joins = false;
    for span in spans {
        let Some(key) = key(span) else {
            joins = false;
            continue;
        };
        match out.last_mut() {
            Some(last) if joins && last.0 == key && (last.3 - span.from).abs() < 1e-6 => {
                last.3 = span.to;
            }
            _ => out.push((key, span, span.from, span.to)),
        }
        joins = true;
    }
    out
}

fn draw(out: &mut Vec<u8>, mark: &Mark) -> Result<()> {
    if mark.to - mark.from <= 0.0 || mark.width <= 0.0 {
        return Ok(());
    }
    colour(out, mark.rgb)?;
    out.extend_from_slice(b"RG ");
    write_number(out, mark.width)?;
    out.extend_from_slice(b" w ");
    match mark.pattern {
        Pattern::Dashed(lengths) => {
            out.push(b'[');
            for (at, length) in lengths.iter().enumerate() {
                if at > 0 {
                    out.push(b' ');
                }
                write_number(out, *length)?;
            }
            out.extend_from_slice(b"] ");
            // The pattern runs from the page's left edge, so stretches of
            // one underline keep its rhythm.
            write_number(out, mark.from)?;
            out.extend_from_slice(b" d ");
        }
        _ => out.extend_from_slice(b"[] 0 d "),
    }
    match mark.pattern {
        Pattern::Wave { height, clip } => wave(out, mark, height, clip),
        _ => {
            numbers(out, [mark.from, mark.y])?;
            out.extend_from_slice(b"m ");
            numbers(out, [mark.to, mark.y])?;
            out.extend_from_slice(b"l S\n");
            Ok(())
        }
    }
}

/// A wave from `from` to `to` about `y`, `height` from peak to trough,
/// clipped to the underline's extent and a band `band` high: whole half
/// waves, the last one cut by the clip.
fn wave(out: &mut Vec<u8>, mark: &Mark, height: f64, band: f64) -> Result<()> {
    let half = WAVE_PERIOD / 2.0;
    // A cubic's control points 4/3 out put its peak at the wave's height.
    let lift = height / 2.0 * 4.0 / 3.0;
    out.extend_from_slice(b"q ");
    numbers(
        out,
        [mark.from, mark.y - band / 2.0, mark.to - mark.from, band],
    )?;
    out.extend_from_slice(b"re W n ");
    numbers(out, [mark.from, mark.y])?;
    out.extend_from_slice(b"m ");
    let mut x = mark.from;
    let mut up = true;
    while x < mark.to - 1e-6 {
        let y = if up { mark.y + lift } else { mark.y - lift };
        numbers(
            out,
            [x + half / 3.0, y, x + 2.0 * half / 3.0, y, x + half, mark.y],
        )?;
        out.extend_from_slice(b"c ");
        x += half;
        up = !up;
    }
    out.extend_from_slice(b"S Q\n");
    Ok(())
}

fn numbers<const N: usize>(out: &mut Vec<u8>, values: [f64; N]) -> Result<()> {
    for value in values {
        write_number(out, value)?;
        out.push(b' ');
    }
    Ok(())
}

pub(super) fn colour(out: &mut Vec<u8>, rgb: [u8; 3]) -> Result<()> {
    numbers(out, rgb.map(|value| f64::from(value) / 255.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Word for Mac 16.115's bands, top to bottom below the baseline, on
    /// Inter 11 pt and Times New Roman 11 and 16 pt (`pr-proof/pr65`).
    #[test]
    fn underline_shapes_follow_word_in_two_faces() {
        type Bands = &'static [(f64, f64)];
        // Size, `post` top and thickness in points, then Word's single,
        // double, thick and wave (the wave's path) bands.
        const FACES: [(&str, f64, f64, f64, [Bands; 4]); 3] = [
            (
                "Inter 11",
                11.0,
                1.869,
                0.752,
                [
                    &[(1.92, 2.64)],
                    &[(0.72, 1.20), (2.16, 2.64)],
                    &[(0.72, 2.16)],
                    &[(0.84, 1.56)],
                ],
            ),
            (
                "Times New Roman 11",
                11.0,
                1.198,
                0.537,
                [
                    &[(1.20, 1.68)],
                    &[(0.72, 0.96), (1.92, 2.16)],
                    &[(0.72, 1.92)],
                    &[(0.84, 1.32)],
                ],
            ),
            (
                "Times New Roman 16",
                16.0,
                1.742,
                0.781,
                [
                    &[(1.68, 2.40)],
                    &[(0.96, 1.44), (2.40, 2.88)],
                    &[(0.96, 2.40)],
                    &[(1.08, 1.80)],
                ],
            ),
        ];
        let styles = [
            UnderlineStyle::Single,
            UnderlineStyle::Double,
            UnderlineStyle::Thick,
            UnderlineStyle::Wave,
        ];
        for (face, size, top, thickness, word) in FACES {
            let plain = Mark {
                rgb: [0; 3],
                from: 0.0,
                to: 100.0,
                y: -top - thickness / 2.0,
                width: thickness,
                pattern: Pattern::Solid,
            };
            for (style, word) in styles.into_iter().zip(word) {
                let mut marks = Vec::new();
                underline(&mut marks, style, plain, -SHAPE_TOP * size);
                let ours: Vec<(f64, f64)> = marks
                    .iter()
                    .map(|mark| {
                        let half = match mark.pattern {
                            Pattern::Wave { height, .. } => height / 2.0,
                            _ => mark.width / 2.0,
                        };
                        (-mark.y - half, -mark.y + half)
                    })
                    .collect();
                let near = ours.len() == word.len()
                    && ours
                        .iter()
                        .zip(word.iter())
                        .all(|(a, b)| (a.0 - b.0).abs() <= 0.25 && (a.1 - b.1).abs() <= 0.25);
                assert!(near, "{face} {style:?}: ours {ours:?}, Word {word:?}");
            }
            for (style, lengths) in [
                (UnderlineStyle::Dotted, &DOTTED[..]),
                (UnderlineStyle::DashedHeavy, &DASH[..]),
                (UnderlineStyle::DashLong, &DASH_LONG[..]),
                (UnderlineStyle::DotDash, &DOT_DASH[..]),
                (UnderlineStyle::DotDotDash, &DOT_DOT_DASH[..]),
            ] {
                let mut marks = Vec::new();
                underline(&mut marks, style, plain, -SHAPE_TOP * size);
                assert_eq!(
                    marks[0].pattern,
                    Pattern::Dashed(lengths),
                    "{face} {style:?}"
                );
            }
        }
    }

    #[test]
    fn a_large_wave_is_not_cut_by_its_band() {
        // Inter at 36 pt: a 2.45 pt line, wider than Word's 11 pt bands.
        let thickness = 0.068 * 36.0;
        let plain = Mark {
            rgb: [0; 3],
            from: 0.0,
            to: 100.0,
            y: -3.0,
            width: thickness,
            pattern: Pattern::Solid,
        };
        for style in [
            UnderlineStyle::Wave,
            UnderlineStyle::WavyHeavy,
            UnderlineStyle::WavyDouble,
        ] {
            let mut marks = Vec::new();
            underline(&mut marks, style, plain, -SHAPE_TOP * 36.0);
            for mark in marks {
                let Pattern::Wave { height, clip } = mark.pattern else {
                    panic!("{style:?}: {mark:?}");
                };
                assert!(clip >= height + mark.width - 1e-9, "{style:?}: {mark:?}");
            }
        }
        // At 11 pt Word's bands hold.
        let mut marks = Vec::new();
        let plain = Mark {
            width: 0.752,
            ..plain
        };
        underline(&mut marks, UnderlineStyle::Wave, plain, -SHAPE_TOP * 11.0);
        assert_eq!(
            marks[0].pattern,
            Pattern::Wave {
                height: 0.752,
                clip: WAVE_CLIP
            }
        );
    }

    #[test]
    fn auto_turns_white_below_word_s_luma() {
        let rgb = |hex: u32| Rgb {
            r: (hex >> 16) as u8,
            g: (hex >> 8) as u8,
            b: hex as u8,
        };
        // Word for Mac 16.115 on run and paragraph shading.
        for (fill, white) in [
            (0x4A4A4A, true),
            (0x4C4C4C, false),
            (0xC00000, true),
            (0xFF0000, false),
            (0x008000, false),
            (0x800080, true),
            (0x1F3864, true),
            (0x7030A0, false),
        ] {
            assert_eq!(dark(rgb(fill)), white, "{fill:06X}");
        }
    }
}
