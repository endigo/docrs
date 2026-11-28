//! Fonts: each face a document draws with becomes a `Type0` font with
//! `Identity-H`, holding a subset of the glyphs it drew and a `ToUnicode`
//! CMap so the text extracts as it was written. The built-in symbol font
//! is a `Type3` font, and a face drawn without its program a simple
//! `TrueType` font referenced by name.

mod face;
mod program;
#[cfg(any(test, feature = "test-support"))]
mod sample;
mod shapes;
mod to_unicode;
mod unembedded;

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use sha2::{Digest, Sha256};
use subsetter::GlyphRemapper;

use self::face::{FaceInfo, Instance, Outlines};
use self::program::Advances;
use super::writer::{Dict, Object, Ref, Writer, count, name, text};
use crate::layout::fonts::Face;
use crate::{Error, Result};

#[cfg(any(test, feature = "test-support"))]
pub use self::sample::SampleLine;
#[cfg(any(test, feature = "test-support"))]
pub(crate) use self::sample::{glyphs_by_cmap, text_document};
pub(crate) use self::shapes::Shape;
use self::shapes::Shapes;
use self::unembedded::Unembedded;
pub(crate) use self::unembedded::code as win_ansi;
pub use crate::layout::variation::Style;

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

/// A font a run is drawn in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PdfFont {
    Embedded(FaceRef),
    /// The built-in symbol font.
    Shapes,
    /// Referenced by name, at its index among them.
    Named(usize),
}

impl PdfFont {
    /// Its name in a page's `/Font` resources.
    pub(crate) fn resource_name(self) -> String {
        match self {
            PdfFont::Embedded(face) => face.resource_name(),
            PdfFont::Shapes => "S1".to_string(),
            PdfFont::Named(at) => format!("N{}", at + 1),
        }
    }

    /// Bytes per character code.
    pub(crate) fn code_bytes(self) -> usize {
        match self {
            PdfFont::Embedded(_) => 2,
            PdfFont::Shapes | PdfFont::Named(_) => 1,
        }
    }
}

/// The fonts of one document, by font file, face and variation instance.
#[derive(Default)]
pub(crate) struct FontResources {
    files: Vec<SourceFile>,
    /// Each file's own allocation (held by `files`, so its address stays
    /// its own), by address and length.
    seen: HashMap<(usize, usize), usize>,
    infos: HashMap<(usize, u32), Arc<FaceInfo>>,
    refs: HashMap<InstanceKey, FaceRef>,
    faces: Vec<Resource>,
    shapes: Shapes,
    named: Vec<Unembedded>,
}

/// File, face index, and each axis value away from the default (as bits).
type InstanceKey = (usize, u32, Vec<([u8; 4], u32)>);

/// A font file, once per content.
struct SourceFile {
    data: Arc<[u8]>,
    digest: [u8; 32],
}

struct Resource {
    file: usize,
    index: u32,
    info: Arc<FaceInfo>,
    instance: Instance,
    /// Source glyphs in the order they were first used; unused when the
    /// face may not be subset.
    remapper: GlyphRemapper,
    /// CID to the source glyph and the text it was first drawn for.
    glyphs: BTreeMap<Cid, (u16, String)>,
    /// Source glyph to each text it was drawn for, with its CID.
    cids: HashMap<u16, Vec<(String, Cid)>>,
}

impl FontResources {
    /// Face `index` of a font file at the instance `style` selects (`None`:
    /// the default instance), as layout measured it. The face
    /// must read as layout reads fonts (`font_malformed`,
    /// `font_cmap_invalid`, `font_too_large`, …), and its licence must allow
    /// embedding (`pdf_font_embedding_restricted`). A file is read once. The
    /// first allocation of each content is kept and recognised by address;
    /// another copy is hashed on every call, so the painter should pass the
    /// `FontSet`'s own `Arc`.
    pub(crate) fn add_face(
        &mut self,
        data: Arc<[u8]>,
        index: u32,
        style: Option<Style>,
    ) -> Result<FaceRef> {
        let file = self.file(data);
        let info = self.info(file, index)?;
        let instance = info.instance(style);
        let key = (
            file,
            index,
            instance
                .variation
                .iter()
                .map(|(tag, value)| (*tag, value.to_bits()))
                .collect(),
        );
        if let Some(known) = self.refs.get(&key) {
            return Ok(*known);
        }
        let face = FaceRef(self.faces.len());
        self.faces.push(Resource {
            file,
            index,
            info,
            instance,
            remapper: GlyphRemapper::new(),
            glyphs: BTreeMap::new(),
            cids: HashMap::new(),
        });
        self.refs.insert(key, face);
        Ok(face)
    }

    /// The file `data` holds: known by its allocation, else by its digest.
    fn file(&mut self, data: Arc<[u8]>) -> usize {
        let address = (Arc::as_ptr(&data) as *const u8 as usize, data.len());
        if let Some(file) = self.seen.get(&address) {
            return *file;
        }
        let digest: [u8; 32] = Sha256::digest(&data).into();
        if let Some(file) = self.files.iter().position(|file| file.digest == digest) {
            return file;
        }
        self.seen.insert(address, self.files.len());
        self.files.push(SourceFile { data, digest });
        self.files.len() - 1
    }

    fn info(&mut self, file: usize, index: u32) -> Result<Arc<FaceInfo>> {
        if let Some(info) = self.infos.get(&(file, index)) {
            return Ok(info.clone());
        }
        let data = &self.files[file].data;
        Face::read_at(data, index as usize)?;
        let info = Arc::new(FaceInfo::read(data, index)?);
        self.infos.insert((file, index), info.clone());
        Ok(info)
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
        let known = face.cids.get(&glyph).and_then(|texts| {
            texts
                .iter()
                .find(|(drawn, _)| drawn == text)
                .map(|(_, cid)| *cid)
        });
        if let Some(cid) = known {
            return Ok(cid);
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
        face.cids
            .entry(glyph)
            .or_default()
            .push((text.to_string(), cid));
        Ok(cid)
    }

    /// The code that draws `shape` with the built-in font and extracts as
    /// `text`, and its width in thousandths of an em.
    pub(crate) fn use_shape(&mut self, shape: Shape, text: &str) -> Result<(u8, f64)> {
        Ok((self.shapes.code(shape, text)?, shape.width()))
    }

    /// The index of `family` in a style, referenced by name and measured
    /// by `face` ([`PdfFont::Named`]).
    pub(crate) fn named(
        &mut self,
        family: &str,
        bold: bool,
        italic: bool,
        face: &Arc<Face>,
    ) -> usize {
        if let Some(at) = self
            .named
            .iter()
            .position(|font| font.is(family, bold, italic))
        {
            return at;
        }
        self.named
            .push(Unembedded::new(family, bold, italic, face.clone()));
        self.named.len() - 1
    }

    /// The width `code` is drawn with in a font referenced by name.
    pub(crate) fn named_width(&self, font: usize, code: u8) -> f64 {
        self.named[font].width(code)
    }

    /// Writes every font's objects, returning each resource name with its
    /// font.
    pub(crate) fn finish(self, writer: &mut Writer) -> Result<Vec<(String, Ref)>> {
        let mut out = Vec::new();
        for (index, face) in self.faces.iter().enumerate() {
            let data = &self.files[face.file].data;
            out.push((FaceRef(index).resource_name(), face.write(data, writer)?));
        }
        if !self.shapes.is_empty() {
            out.push((PdfFont::Shapes.resource_name(), self.shapes.write(writer)?));
        }
        for (at, font) in self.named.iter().enumerate() {
            out.push((PdfFont::Named(at).resource_name(), font.write(writer)?));
        }
        Ok(out)
    }
}

impl Resource {
    fn write(&self, data: &[u8], writer: &mut Writer) -> Result<Ref> {
        let program = program::embed(
            data,
            self.index,
            &self.info,
            &self.instance.variation,
            &self.remapper,
        )?;
        let advances = Advances::read(&program)?;
        let cff = self.info.outlines == Outlines::Cff;
        let base_font = self.base_font();
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
        writer.stream(to_unicode, Dict::new(), &to_unicode::cmap(texts, 2))?;
        // ISO 32000-1 9.7.6.1: a Type0 font over a CIDFontType0 font is
        // named after it and its CMap.
        let type0_name = if cff {
            format!("{base_font}-Identity-H")
        } else {
            base_font
        };
        writer.dict(
            type0,
            Dict::new()
                .with("Type", name("Font"))
                .with("Subtype", name("Type0"))
                .with("BaseFont", name(&type0_name))
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

    /// The PostScript name, after a subset tag when the program is a subset.
    fn base_font(&self) -> String {
        if self.info.subsettable {
            format!("{}+{}", self.tag(), self.instance.postscript_name)
        } else {
            self.instance.postscript_name.clone()
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

    /// The bbox, and the italic angle of an `ital` instance, are the default
    /// instance's: subsetter keeps the source `head` and `post` tables.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_first_allocation_of_a_file_is_held() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/fonts/liberation/LiberationSerif-Regular.ttf"
        );
        let first: Arc<[u8]> = std::fs::read(path).unwrap().into();
        let mut fonts = FontResources::default();
        let face = fonts.add_face(first.clone(), 0, None).unwrap();
        for _ in 0..3 {
            let copy: Arc<[u8]> = first.to_vec().into();
            assert_eq!(fonts.add_face(copy, 0, None).unwrap(), face);
        }
        assert_eq!((fonts.files.len(), fonts.seen.len()), (1, 1));
        assert!(Arc::ptr_eq(&fonts.files[0].data, &first));
        assert_eq!(fonts.add_face(first, 0, None).unwrap(), face);
    }
}
