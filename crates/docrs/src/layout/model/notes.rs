//! Footnotes and endnotes: each note's blocks, the separators Word draws
//! above them, and their numbering (`w:footnotePr`, `w:endnotePr`).

use std::collections::HashMap;

use super::numbering::format_number;
use super::{Block, Builder, StoryId, hyperlinks};
use crate::package::rels::rt;
use crate::txn::Txn;
use crate::wml::{w_attr, w_child};
use crate::xml::{NodeId, XmlTree};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NoteKind {
    Footnote,
    Endnote,
}

impl NoteKind {
    pub fn story(self) -> StoryId {
        match self {
            NoteKind::Footnote => StoryId::Footnotes,
            NoteKind::Endnote => StoryId::Endnotes,
        }
    }

    fn slot(self) -> usize {
        self as usize
    }
}

/// A note: its kind and its place in its part.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NoteId {
    pub kind: NoteKind,
    pub index: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Restart {
    #[default]
    Continuous,
    Section,
    Page,
}

/// Where Word puts a kind of note (`w:pos`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NotePosition {
    #[default]
    PageBottom,
    BeneathText,
    SectionEnd,
    DocumentEnd,
}

/// A section's numbering of one kind of note.
#[derive(Debug, Clone, PartialEq)]
pub struct NoteNumbering {
    pub format: String,
    pub start: u32,
    pub restart: Restart,
    pub position: NotePosition,
}

impl NoteNumbering {
    fn default_for(kind: NoteKind) -> NoteNumbering {
        let (format, position) = match kind {
            NoteKind::Footnote => ("decimal", NotePosition::PageBottom),
            NoteKind::Endnote => ("lowerRoman", NotePosition::DocumentEnd),
        };
        NoteNumbering {
            format: format.to_string(),
            start: 1,
            restart: Restart::Continuous,
            position,
        }
    }

    /// `w:footnotePr` or `w:endnotePr` over these.
    fn over(&self, tree: &XmlTree, pr: Option<NodeId>) -> NoteNumbering {
        let mut out = self.clone();
        let Some(pr) = pr else { return out };
        let val = |name: &str| w_child(tree, pr, name).and_then(|n| w_attr(tree, n, "val"));
        if let Some(format) = val("numFmt") {
            out.format = format.to_string();
        }
        if let Some(start) = val("numStart").and_then(|v| v.parse().ok()) {
            out.start = start;
        }
        match val("numRestart") {
            Some("eachSect") => out.restart = Restart::Section,
            Some("eachPage") => out.restart = Restart::Page,
            Some("continuous") => out.restart = Restart::Continuous,
            _ => {}
        }
        match val("pos") {
            Some("pageBottom") => out.position = NotePosition::PageBottom,
            Some("beneathText") => out.position = NotePosition::BeneathText,
            Some("sectEnd") => out.position = NotePosition::SectionEnd,
            Some("docEnd") => out.position = NotePosition::DocumentEnd,
            _ => {}
        }
        out
    }

    pub fn mark(&self, number: u32) -> String {
        format_number(i64::from(number), &self.format)
    }
}

impl Default for NoteSettings {
    fn default() -> Self {
        NoteSettings::document(None)
    }
}

/// Both kinds' numbering for a section.
#[derive(Debug, Clone, PartialEq)]
pub struct NoteSettings(pub [NoteNumbering; 2]);

impl NoteSettings {
    pub fn get(&self, kind: NoteKind) -> &NoteNumbering {
        &self.0[kind.slot()]
    }

    /// The settings part's, else Word's defaults.
    pub(crate) fn document(settings: Option<&XmlTree>) -> NoteSettings {
        let read = |kind: NoteKind, name: &str| {
            let default = NoteNumbering::default_for(kind);
            match settings {
                Some(s) => default.over(s, w_child(s, s.root(), name)),
                None => default,
            }
        };
        NoteSettings([
            read(NoteKind::Footnote, "footnotePr"),
            read(NoteKind::Endnote, "endnotePr"),
        ])
    }

    /// A section's own over the document's.
    pub(crate) fn section(&self, tree: &XmlTree, sect: NodeId) -> NoteSettings {
        NoteSettings([
            self.0[0].over(tree, w_child(tree, sect, "footnotePr")),
            self.0[1].over(tree, w_child(tree, sect, "endnotePr")),
        ])
    }
}

/// The next number of each kind of note, restarted as the section says.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoteCounter {
    next: [u32; 2],
}

impl NoteCounter {
    pub fn new(first: &NoteSettings) -> NoteCounter {
        NoteCounter {
            next: [first.0[0].start, first.0[1].start],
        }
    }

    pub fn section(&mut self, settings: &NoteSettings) {
        self.restart(settings, Restart::Section);
    }

    pub fn page(&mut self, settings: &NoteSettings) {
        self.restart(settings, Restart::Page);
    }

    fn restart(&mut self, settings: &NoteSettings, at: Restart) {
        for (next, numbering) in self.next.iter_mut().zip(&settings.0) {
            if numbering.restart == at {
                *next = numbering.start;
            }
        }
    }

    pub fn take(&mut self, kind: NoteKind) -> u32 {
        let next = &mut self.next[kind.slot()];
        let number = *next;
        *next = next.saturating_add(1);
        number
    }
}

#[derive(Debug, Clone, Default)]
pub struct Note {
    pub blocks: Vec<Block>,
    /// Marked by the text after its reference (`w:customMarkFollows`), not
    /// numbered.
    pub custom: bool,
}

/// One kind of note, in its part's order, and the separators.
#[derive(Debug, Clone, Default)]
pub struct NoteStory {
    pub notes: Vec<Note>,
    pub separator: Vec<Block>,
    pub continuation: Vec<Block>,
}

#[derive(Debug, Clone, Default)]
pub struct Notes {
    pub footnotes: NoteStory,
    pub endnotes: NoteStory,
}

impl Notes {
    pub fn story(&self, kind: NoteKind) -> &NoteStory {
        match kind {
            NoteKind::Footnote => &self.footnotes,
            NoteKind::Endnote => &self.endnotes,
        }
    }

    pub fn note(&self, id: NoteId) -> Option<&Note> {
        self.story(id.kind).notes.get(id.index as usize)
    }
}

/// A notes part: its tree, its notes by `w:id` and its separators.
pub(super) struct NotePart<'t> {
    tree: &'t XmlTree,
    hyperlinks: HashMap<String, String>,
    ids: HashMap<String, u32>,
    notes: Vec<NodeId>,
    separator: Option<NodeId>,
    continuation: Option<NodeId>,
}

impl<'t> NotePart<'t> {
    fn read(tree: &'t XmlTree, hyperlinks: HashMap<String, String>) -> NotePart<'t> {
        let mut part = NotePart {
            tree,
            hyperlinks,
            ids: HashMap::new(),
            notes: Vec::new(),
            separator: None,
            continuation: None,
        };
        for note in tree.child_elements(tree.root()) {
            match w_attr(tree, note, "type") {
                Some("separator") => part.separator = Some(note),
                Some("continuationSeparator") => part.continuation = Some(note),
                None | Some("normal") => {
                    if let Some(id) = w_attr(tree, note, "id") {
                        part.ids.insert(id.to_string(), part.notes.len() as u32);
                        part.notes.push(note);
                    }
                }
                _ => {}
            }
        }
        part
    }
}

/// What the builder knows of notes while it reads the stories.
pub(super) struct NoteState<'t> {
    parts: [Option<NotePart<'t>>; 2],
    /// Each section's numbering, in body order.
    sections: Vec<NoteSettings>,
    counter: NoteCounter,
    counted_section: usize,
    /// Each note's mark, from its number in document order.
    marks: [Vec<Option<String>>; 2],
    custom: [Vec<bool>; 2],
    /// The note being read.
    current: Option<NoteId>,
}

impl<'t> NoteState<'t> {
    pub(super) fn read(txn: &'t Txn<'_>, main: &str, sections: Vec<NoteSettings>) -> Self {
        let read = |rel: &str| {
            let name = txn.part_related_by(main, rel).ok().flatten()?;
            let tree = txn.tree(&name).ok()?;
            Some(NotePart::read(tree, hyperlinks(txn, &name)))
        };
        let parts = [read(rt::FOOTNOTES), read(rt::ENDNOTES)];
        let sized = |slot: usize| parts[slot].as_ref().map_or(0, |p| p.notes.len());
        NoteState {
            marks: [vec![None; sized(0)], vec![None; sized(1)]],
            custom: [vec![false; sized(0)], vec![false; sized(1)]],
            counter: sections.first().map(NoteCounter::new).unwrap_or_default(),
            counted_section: 0,
            sections,
            parts,
            current: None,
        }
    }

    fn settings(&self, section: usize) -> Option<&NoteSettings> {
        self.sections.get(section).or_else(|| self.sections.last())
    }

    /// The note `w:id` names and its mark, numbered in document order on
    /// the first reference (Word renumbers per page; layout measures the
    /// marks once).
    fn reference(
        &mut self,
        kind: NoteKind,
        id: &str,
        custom: bool,
        section: usize,
    ) -> Option<(NoteId, String)> {
        let slot = kind.slot();
        let index = *self.parts[slot].as_ref()?.ids.get(id)?;
        while self.counted_section < section {
            self.counted_section += 1;
            if let Some(settings) = self.settings(self.counted_section).cloned() {
                self.counter.section(&settings);
            }
        }
        let note = NoteId { kind, index };
        if custom {
            self.custom[slot][index as usize] = true;
        }
        if self.marks[slot][index as usize].is_none() {
            let mark = if custom {
                String::new()
            } else {
                let number = self.counter.take(kind);
                self.settings(section)
                    .map_or_else(|| number.to_string(), |s| s.get(kind).mark(number))
            };
            self.marks[slot][index as usize] = Some(mark);
        }
        Some((note, self.mark(note)))
    }

    fn mark(&self, note: NoteId) -> String {
        self.marks[note.kind.slot()]
            .get(note.index as usize)
            .cloned()
            .flatten()
            .unwrap_or_default()
    }
}

impl Builder<'_> {
    /// `w:footnoteReference`/`w:endnoteReference`, or (in a note's own
    /// text) `w:footnoteRef`/`w:endnoteRef`: the note and its mark.
    pub(super) fn note_mark(&mut self, tree: &XmlTree, node: NodeId) -> Option<(NoteId, String)> {
        let local = tree.local_name(node)?;
        let kind = if local.starts_with("footnote") {
            NoteKind::Footnote
        } else {
            NoteKind::Endnote
        };
        if local.ends_with("Ref") {
            let note = self.notes.current.filter(|n| n.kind == kind)?;
            return Some((note, self.notes.mark(note)));
        }
        if self.story != StoryId::Body {
            return None;
        }
        let id = w_attr(tree, node, "id")?;
        let custom = w_attr(tree, node, "customMarkFollows")
            .is_some_and(|v| matches!(v, "1" | "true" | "on"));
        let section = self.sections_seen;
        self.notes.reference(kind, id, custom, section)
    }

    /// Both notes parts, read after the body so the marks are numbered.
    pub(super) fn note_stories(&mut self) -> Notes {
        let mut stories = [NoteStory::default(), NoteStory::default()];
        for kind in [NoteKind::Footnote, NoteKind::Endnote] {
            let slot = kind.slot();
            let Some(part) = self.notes.parts[slot].take() else {
                continue;
            };
            self.story = kind.story();
            self.hyperlinks = part.hyperlinks;
            let tree = part.tree;
            let story = &mut stories[slot];
            story.separator = self.note_blocks(tree, part.separator);
            story.continuation = self.note_blocks(tree, part.continuation);
            for (index, node) in part.notes.iter().enumerate() {
                self.notes.current = Some(NoteId {
                    kind,
                    index: index as u32,
                });
                story.notes.push(Note {
                    blocks: self.note_blocks(tree, Some(*node)),
                    custom: self.notes.custom[slot][index],
                });
            }
            self.notes.current = None;
        }
        let [footnotes, endnotes] = stories;
        Notes {
            footnotes,
            endnotes,
        }
    }

    fn note_blocks(&mut self, tree: &XmlTree, note: Option<NodeId>) -> Vec<Block> {
        let mut blocks = Vec::new();
        if let Some(note) = note {
            self.blocks(tree, note, None, &mut blocks);
        }
        blocks
    }
}
