//! Embedded fonts: each face a document draws with becomes a `Type0` font
//! with `Identity-H`, holding a subset of the glyphs it drew and a
//! `ToUnicode` CMap so the text extracts as it was written.

mod face;
mod program;
#[cfg(any(test, feature = "test-support"))]
mod sample;
mod to_unicode;

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use sha2::{Digest, Sha256};
use subsetter::GlyphRemapper;

use self::face::{FaceInfo, Instance, Outlines};
use self::program::Advances;
use super::writer::{Dict, Object, Ref, Writer, count, name, text};
use crate::{Error, Result};

pub use self::face::Style;
#[cfg(any(test, feature = "test-support"))]
pub use self::sample::SampleLine;
#[cfg(any(test, feature = "test-support"))]
pub(crate) use self::sample::{glyphs_by_cmap, text_document};

/// A character code in an embedded font's content stream (two bytes, `Identity-H`).
pub(crate) type Cid = u16;

/// A face added to [`FontResources`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FaceRef(usize);

impl FaceRef {
    /// The face's name in a page's `/Font` resources.
    pub(crate) fn resource_name(self) -> String {
        format!("F{}", self.0 + 1)
    }
}

/// The fonts of one document, by font file, face and variation instance.
#[derive(Default)]
pub(crate) struct FontResources {
    faces: Vec<Resource>,
}

struct Resource {
    data: Arc<[u8]>,
    index: u32,
    info: FaceInfo,
    instance: Instance,
    /// Source glyphs in the order they were first used; unused when the
    /// face may not be subset.
    remapper: GlyphRemapper,
    /// CID to the source glyph and the text it was first drawn for.
    glyphs: BTreeMap<Cid, (u16, String)>,
    cids: HashMap<(u16, String), Cid>,
}

impl FontResources {
    /// Face `index` of a font file at the instance `style` selects. Refuses
    /// a face whose licence forbids embedding (`pdf_font_embedding_restricted`).
    pub(crate) fn add_face(
        &mut self,
        data: Arc<[u8]>,
        index: u32,
        style: Style,
    ) -> Result<FaceRef> {
        let info = FaceInfo::read(&data, index)?;
        let instance = info.instance(style);
        let known = self.faces.iter().position(|face| {
            face.index == index
                && face.instance.variation == instance.variation
                && (Arc::ptr_eq(&face.data, &data) || face.data == data)
        });
        if let Some(known) = known {
            return Ok(FaceRef(known));
        }
        self.faces.push(Resource {
            data,
            index,
            info,
            instance,
            remapper: GlyphRemapper::new(),
            glyphs: BTreeMap::new(),
            cids: HashMap::new(),
        });
        Ok(FaceRef(self.faces.len() - 1))
    }

    /// The CID that draws glyph `glyph` of `face` and extracts as `text`.
    /// A TrueType face gives each (glyph, text) pair its own CID, so a space
    /// and a no-break space sharing a glyph extract apart. A CFF face's CID
    /// is its glyph id in the embedded program, so a glyph drawn for two
    /// texts extracts as the first.
    pub(crate) fn use_glyph(&mut self, face: FaceRef, glyph: u16, text: &str) -> Result<Cid> {
        let face = &mut self.faces[face.0];
        if glyph >= face.info.glyphs {
            return Err(Error::invalid_argument(
                "pdf_glyph_out_of_range",
                format!("{} has no glyph {glyph}", face.info.label),
            ));
        }
        if let Some(cid) = face.cids.get(&(glyph, text.to_string())) {
            return Ok(*cid);
        }
        let cid = match face.info.outlines {
            Outlines::Cff => face.keep(glyph),
            Outlines::TrueType | Outlines::Cff2 => {
                let cid = Cid::try_from(face.glyphs.len() + 1).map_err(|_| {
                    Error::invalid_argument(
                        "pdf_font_too_many_glyphs",
                        format!("{} draws more than 65,535 distinct glyphs", face.info.label),
                    )
                })?;
                face.keep(glyph);
                cid
            }
        };
        face.glyphs
            .entry(cid)
            .or_insert_with(|| (glyph, text.to_string()));
        face.cids.insert((glyph, text.to_string()), cid);
        Ok(cid)
    }

    /// Writes every face's font objects, returning each resource name with
    /// its `Type0` font.
    pub(crate) fn finish(self, writer: &mut Writer) -> Result<Vec<(String, Ref)>> {
        self.faces
            .iter()
            .enumerate()
            .map(|(index, face)| Ok((FaceRef(index).resource_name(), face.write(writer)?)))
            .collect()
    }
}

impl Resource {
    fn write(&self, writer: &mut Writer) -> Result<Ref> {
        let program = program::embed(
            &self.data,
            self.index,
            &self.info,
            &self.instance.variation,
            &self.remapper,
        )?;
        let advances = Advances::read(&program)?;
        let cff = self.info.outlines == Outlines::Cff;
        let base_font = format!("{}+{}", self.tag(), self.instance.postscript_name);
        let type0 = writer.alloc()?;
        let cid_font = writer.alloc()?;
        let descriptor = writer.alloc()?;
        let file = writer.alloc()?;
        let to_unicode = writer.alloc()?;
        let mut file_dict = Dict::new();
        if cff {
            file_dict.set("Subtype", name("OpenType"));
        } else {
            file_dict.set("Length1", count(program.len())?);
        }
        writer.stream(file, file_dict, &program)?;
        writer.dict(descriptor, self.descriptor(&base_font, cff, file))?;
        let mut cid_dict = Dict::new()
            .with("Type", name("Font"))
            .with(
                "Subtype",
                name(if cff { "CIDFontType0" } else { "CIDFontType2" }),
            )
            .with("BaseFont", name(&base_font))
            .with(
                "CIDSystemInfo",
                Dict::new()
                    .with("Registry", text("Adobe"))
                    .with("Ordering", text("Identity"))
                    .with("Supplement", 0_i64),
            )
            .with("FontDescriptor", descriptor)
            .with("W", self.widths(&advances));
        if !cff {
            let map = writer.alloc()?;
            writer.stream(map, Dict::new(), &self.cid_to_gid())?;
            cid_dict.set("CIDToGIDMap", map);
        }
        writer.dict(cid_font, cid_dict)?;
        let texts = self
            .glyphs
            .iter()
            .map(|(cid, (_, text))| (*cid, text.as_str()));
        writer.stream(to_unicode, Dict::new(), &to_unicode::cmap(texts))?;
        writer.dict(
            type0,
            Dict::new()
                .with("Type", name("Font"))
                .with("Subtype", name("Type0"))
                .with("BaseFont", name(&base_font))
                .with("Encoding", name("Identity-H"))
                .with("DescendantFonts", vec![Object::from(cid_font)])
                .with("ToUnicode", to_unicode),
        )?;
        Ok(type0)
    }

    /// Keeps `glyph` in the subset, returning its id in the embedded program.
    fn keep(&mut self, glyph: u16) -> u16 {
        if self.info.subsettable {
            self.remapper.remap(glyph)
        } else {
            glyph
        }
    }

    /// The glyph's id in the embedded program.
    fn embedded(&self, glyph: u16) -> u16 {
        if self.info.subsettable {
            self.remapper.get(glyph).unwrap_or(0)
        } else {
            glyph
        }
    }

    /// Six capitals from a hash of the face, its instance and its glyphs,
    /// so different subsets of one font get different names (ISO 32000-1 9.6.4).
    fn tag(&self) -> String {
        let mut hash = Sha256::new();
        hash.update(self.instance.postscript_name.as_bytes());
        for (axis, value) in &self.instance.variation {
            hash.update(axis);
            hash.update(value.to_be_bytes());
        }
        for (cid, (glyph, _)) in &self.glyphs {
            hash.update(cid.to_be_bytes());
            hash.update(glyph.to_be_bytes());
        }
        hash.finalize()[..6]
            .iter()
            .map(|byte| char::from(b'A' + byte % 26))
            .collect()
    }

    /// `/W`: the advance of each used CID, consecutive CIDs in one array.
    fn widths(&self, advances: &Advances) -> Vec<Object> {
        let mut out = Vec::new();
        let mut run: Option<(Cid, Vec<Object>)> = None;
        for (cid, (glyph, _)) in &self.glyphs {
            let width = Object::Real(advances.width(self.embedded(*glyph)));
            match &mut run {
                Some((first, widths))
                    if usize::from(*first) + widths.len() == usize::from(*cid) =>
                {
                    widths.push(width);
                }
                _ => {
                    if let Some((first, widths)) = run.take() {
                        out.extend([Object::from(u32::from(first)), widths.into()]);
                    }
                    run = Some((*cid, vec![width]));
                }
            }
        }
        if let Some((first, widths)) = run {
            out.extend([Object::from(u32::from(first)), widths.into()]);
        }
        out
    }

    /// `CIDToGIDMap`: two bytes per CID from 0 to the last one used.
    fn cid_to_gid(&self) -> Vec<u8> {
        let last = self.glyphs.keys().next_back().copied().unwrap_or(0);
        let mut map = vec![0; 2 * (usize::from(last) + 1)];
        for (cid, (glyph, _)) in &self.glyphs {
            let at = 2 * usize::from(*cid);
            map[at..at + 2].copy_from_slice(&self.embedded(*glyph).to_be_bytes());
        }
        map
    }

    fn descriptor(&self, base_font: &str, cff: bool, file: Ref) -> Dict {
        let info = &self.info;
        let instance = &self.instance;
        let scale = |value: f64| (value * 1000.0 / info.units_per_em).round();
        let mut flags = SYMBOLIC;
        if info.fixed_pitch {
            flags |= FIXED_PITCH;
        }
        if info.serif {
            flags |= SERIF;
        }
        if instance.italic {
            flags |= ITALIC;
        }
        let weight = f64::from(instance.weight);
        Dict::new()
            .with("Type", name("FontDescriptor"))
            .with("FontName", name(base_font))
            .with("Flags", flags)
            .with(
                "FontBBox",
                info.bbox.map(|v| Object::Real(scale(v))).to_vec(),
            )
            .with("ItalicAngle", instance.italic_angle)
            .with("Ascent", scale(info.ascent))
            .with("Descent", scale(info.descent))
            .with("CapHeight", scale(info.cap_height))
            .with("StemV", (10.0 + 0.244 * (weight - 50.0)).round().max(0.0))
            .with(
                "FontWeight",
                ((weight / 100.0).round() * 100.0).clamp(100.0, 900.0),
            )
            .with(if cff { "FontFile3" } else { "FontFile2" }, file)
    }
}

/// Font descriptor flags (ISO 32000-1 Table 123). Every embedded font is
/// symbolic: its glyphs are reached by CID, not a standard encoding.
const FIXED_PITCH: u32 = 1;
const SERIF: u32 = 1 << 1;
const SYMBOLIC: u32 = 1 << 2;
const ITALIC: u32 = 1 << 6;
