use std::collections::HashMap;

use super::colour::Palette;
use super::props::{PPr, RPr, read_ppr, read_rpr};
use crate::wml::{w_attr, w_child, w_children};
use crate::xml::{NodeId, XmlTree};

pub(super) struct Level {
    format: String,
    text: String,
    start: i64,
    pub(super) ppr: PPr,
    pub(super) rpr: RPr,
    pub(super) suffix: String,
}

/// A `w:num`: its abstractNumId, level overrides and start overrides.
type Num = (String, HashMap<u8, Level>, HashMap<u8, i64>);

#[derive(Default)]
pub(super) struct Numbering {
    nums: HashMap<String, Num>,
    abstracts: HashMap<String, HashMap<u8, Level>>,
    counters: HashMap<String, [i64; 9]>,
}

fn read_level(tree: &XmlTree, lvl: NodeId, palette: &Palette) -> Level {
    let val = |name: &str| w_child(tree, lvl, name).and_then(|n| w_attr(tree, n, "val"));
    Level {
        format: val("numFmt").unwrap_or("decimal").to_string(),
        text: val("lvlText").unwrap_or("").to_string(),
        start: val("start").and_then(|v| v.parse().ok()).unwrap_or(1),
        ppr: read_ppr(tree, w_child(tree, lvl, "pPr"), palette),
        rpr: read_rpr(tree, w_child(tree, lvl, "rPr"), palette),
        suffix: val("suff").unwrap_or("tab").to_string(),
    }
}

impl Numbering {
    pub(super) fn read(tree: Option<&XmlTree>, palette: &Palette) -> Numbering {
        let mut out = Numbering::default();
        let Some(tree) = tree else {
            return out;
        };
        let root = tree.root();
        for abs in w_children(tree, root, "abstractNum") {
            let Some(id) = w_attr(tree, abs, "abstractNumId") else {
                continue;
            };
            let mut levels = HashMap::new();
            for lvl in w_children(tree, abs, "lvl") {
                if let Some(ilvl) = w_attr(tree, lvl, "ilvl").and_then(|v| v.parse().ok()) {
                    levels.insert(ilvl, read_level(tree, lvl, palette));
                }
            }
            out.abstracts.insert(id.to_string(), levels);
        }
        for num in w_children(tree, root, "num") {
            let Some(id) = w_attr(tree, num, "numId") else {
                continue;
            };
            let abs = w_child(tree, num, "abstractNumId")
                .and_then(|n| w_attr(tree, n, "val"))
                .unwrap_or("")
                .to_string();
            let mut levels = HashMap::new();
            let mut starts = HashMap::new();
            for o in w_children(tree, num, "lvlOverride") {
                let Some(ilvl) = w_attr(tree, o, "ilvl").and_then(|v| v.parse().ok()) else {
                    continue;
                };
                if let Some(start) = w_child(tree, o, "startOverride")
                    .and_then(|s| w_attr(tree, s, "val"))
                    .and_then(|v| v.parse().ok())
                {
                    starts.insert(ilvl, start);
                }
                if let Some(lvl) = w_child(tree, o, "lvl") {
                    levels.insert(ilvl, read_level(tree, lvl, palette));
                }
            }
            out.nums.insert(id.to_string(), (abs, levels, starts));
        }
        out
    }

    pub(super) fn level(&self, num: &str, ilvl: u8) -> Option<&Level> {
        level_in(&self.nums, &self.abstracts, num, ilvl)
    }

    /// Advance the counters and return the label text.
    pub(super) fn label(&mut self, num: &str, ilvl: u8) -> Option<String> {
        let (abs, _, starts) = self.nums.get(num)?;
        let level = level_in(&self.nums, &self.abstracts, num, ilvl)?;
        let mut starts_by_level = [1i64; 9];
        for (i, slot) in starts_by_level.iter_mut().enumerate() {
            *slot = starts
                .get(&(i as u8))
                .copied()
                .or_else(|| level_in(&self.nums, &self.abstracts, num, i as u8).map(|l| l.start))
                .unwrap_or(1);
        }
        let counters = self.counters.entry(abs.clone()).or_insert_with(|| {
            let mut c = [0; 9];
            for (i, v) in c.iter_mut().enumerate() {
                *v = starts_by_level[i] - 1;
            }
            c
        });
        let level_index = usize::from(ilvl.min(8));
        counters[level_index] += 1;
        for deeper in level_index + 1..9 {
            counters[deeper] = starts_by_level[deeper] - 1;
        }
        let values = *counters;
        if level.format == "bullet" {
            return Some(level.text.clone());
        }
        let mut out = String::new();
        let mut chars = level.text.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '%'
                && let Some(d) = chars.peek().and_then(|d| d.to_digit(10))
            {
                chars.next();
                let index = (d as usize).saturating_sub(1).min(8);
                out.push_str(&format_number(values[index], &level.format));
                continue;
            }
            out.push(c);
        }
        Some(out)
    }
}

/// `Numbering::level` over the maps alone, so `label` can hold a level
/// while it advances the counters.
fn level_in<'a>(
    nums: &'a HashMap<String, Num>,
    abstracts: &'a HashMap<String, HashMap<u8, Level>>,
    num: &str,
    ilvl: u8,
) -> Option<&'a Level> {
    let (abs, overrides, _) = nums.get(num)?;
    overrides
        .get(&ilvl)
        .or_else(|| abstracts.get(abs).and_then(|levels| levels.get(&ilvl)))
}

fn format_number(value: i64, format: &str) -> String {
    match format {
        "lowerLetter" | "upperLetter" => {
            let n = value.max(1) - 1;
            let letter = (b'a' + (n % 26) as u8) as char;
            let s: String = std::iter::repeat_n(letter, (n / 26 + 1) as usize).collect();
            if format == "upperLetter" {
                s.to_uppercase()
            } else {
                s
            }
        }
        "lowerRoman" | "upperRoman" => {
            let s = roman(value);
            if format == "upperRoman" {
                s.to_uppercase()
            } else {
                s
            }
        }
        "decimalZero" => format!("{value:02}"),
        "none" => String::new(),
        _ => value.to_string(),
    }
}

fn roman(mut value: i64) -> String {
    let table = [
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ];
    let mut out = String::new();
    for (n, s) in table {
        while value >= n {
            out.push_str(s);
            value -= n;
        }
    }
    out
}
