//! A faithful port of CPython 3.13 `difflib.SequenceMatcher` (no `isjunk`,
//! `autojunk=True`) and `unified_diff`, used by text diffs and compare.
//!
//! Exactness matters: upstream reports and compare redlines are defined by
//! which matching blocks this algorithm picks, including its "popular
//! element" junk heuristic for sequences of 200+ items and the order in
//! which longest matches are extended.

use std::collections::HashMap;
use std::hash::Hash;

/// `(i, j, size)`: `a[i..i+size] == b[j..j+size]`.
pub type Match = (usize, usize, usize);

/// Opcode tags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tag {
    /// `a[i1..i2] == b[j1..j2]`.
    Equal,
    /// `a[i1..i2]` replaced by `b[j1..j2]`.
    Replace,
    /// `a[i1..i2]` deleted.
    Delete,
    /// `b[j1..j2]` inserted.
    Insert,
}

/// `(tag, i1, i2, j1, j2)`.
pub type Opcode = (Tag, usize, usize, usize, usize);

/// `difflib.SequenceMatcher(None, a, b)`.
pub struct SequenceMatcher<'a, T: Eq + Hash> {
    a: &'a [T],
    b: &'a [T],
    b2j: HashMap<&'a T, Vec<usize>>,
}

impl<'a, T: Eq + Hash> SequenceMatcher<'a, T> {
    /// Build the matcher (upstream `__chain_b` with `autojunk=True`).
    pub fn new(a: &'a [T], b: &'a [T]) -> Self {
        Self::with_autojunk(a, b, true)
    }

    /// Build the matcher with an explicit `autojunk` flag.
    pub fn with_autojunk(a: &'a [T], b: &'a [T], autojunk: bool) -> Self {
        let mut b2j: HashMap<&'a T, Vec<usize>> = HashMap::new();
        for (index, element) in b.iter().enumerate() {
            b2j.entry(element).or_default().push(index);
        }
        let n = b.len();
        if autojunk && n >= 200 {
            let ntest = n / 100 + 1;
            let popular: Vec<&'a T> = b2j
                .iter()
                .filter(|(_, indices)| indices.len() > ntest)
                .map(|(element, _)| *element)
                .collect();
            for element in popular {
                b2j.remove(element);
            }
        }
        // Popular elements leave b2j but are not "junk": `bjunk` is empty
        // without `isjunk`, so upstream's junk-extension passes never match.
        SequenceMatcher { a, b, b2j }
    }

    /// Upstream `find_longest_match`.
    pub fn find_longest_match(&self, alo: usize, ahi: usize, blo: usize, bhi: usize) -> Match {
        let (a, b) = (self.a, self.b);
        let (mut besti, mut bestj, mut bestsize) = (alo, blo, 0usize);
        let mut j2len: HashMap<usize, usize> = HashMap::new();
        let mut next: HashMap<usize, usize> = HashMap::new();
        for (i, element) in a.iter().enumerate().take(ahi).skip(alo) {
            if let Some(indices) = self.b2j.get(element) {
                for &j in indices {
                    if j < blo {
                        continue;
                    }
                    if j >= bhi {
                        break;
                    }
                    let k = j
                        .checked_sub(1)
                        .and_then(|p| j2len.get(&p))
                        .copied()
                        .unwrap_or(0)
                        + 1;
                    next.insert(j, k);
                    if k > bestsize {
                        besti = i + 1 - k;
                        bestj = j + 1 - k;
                        bestsize = k;
                    }
                }
            }
            std::mem::swap(&mut j2len, &mut next);
            next.clear();
        }
        while besti > alo && bestj > blo && a[besti - 1] == b[bestj - 1] {
            besti -= 1;
            bestj -= 1;
            bestsize += 1;
        }
        while besti + bestsize < ahi
            && bestj + bestsize < bhi
            && a[besti + bestsize] == b[bestj + bestsize]
        {
            bestsize += 1;
        }
        (besti, bestj, bestsize)
    }

    /// Upstream `get_matching_blocks` (ends with the `(len a, len b, 0)` sentinel).
    pub fn matching_blocks(&self) -> Vec<Match> {
        let (la, lb) = (self.a.len(), self.b.len());
        let mut queue = vec![(0, la, 0, lb)];
        let mut blocks: Vec<Match> = Vec::new();
        while let Some((alo, ahi, blo, bhi)) = queue.pop() {
            let (i, j, k) = self.find_longest_match(alo, ahi, blo, bhi);
            if k > 0 {
                blocks.push((i, j, k));
                if alo < i && blo < j {
                    queue.push((alo, i, blo, j));
                }
                if i + k < ahi && j + k < bhi {
                    queue.push((i + k, ahi, j + k, bhi));
                }
            }
        }
        blocks.sort();
        let (mut i1, mut j1, mut k1) = (0, 0, 0);
        let mut out = Vec::new();
        for (i2, j2, k2) in blocks {
            if i1 + k1 == i2 && j1 + k1 == j2 {
                k1 += k2;
            } else {
                if k1 > 0 {
                    out.push((i1, j1, k1));
                }
                i1 = i2;
                j1 = j2;
                k1 = k2;
            }
        }
        if k1 > 0 {
            out.push((i1, j1, k1));
        }
        out.push((la, lb, 0));
        out
    }

    /// Upstream `get_opcodes`.
    pub fn opcodes(&self) -> Vec<Opcode> {
        let (mut i, mut j) = (0, 0);
        let mut out = Vec::new();
        for (ai, bj, size) in self.matching_blocks() {
            let tag = if i < ai && j < bj {
                Some(Tag::Replace)
            } else if i < ai {
                Some(Tag::Delete)
            } else if j < bj {
                Some(Tag::Insert)
            } else {
                None
            };
            if let Some(tag) = tag {
                out.push((tag, i, ai, j, bj));
            }
            i = ai + size;
            j = bj + size;
            if size > 0 {
                out.push((Tag::Equal, ai, i, bj, j));
            }
        }
        out
    }

    /// Upstream `get_grouped_opcodes(n)`.
    pub fn grouped_opcodes(&self, n: usize) -> Vec<Vec<Opcode>> {
        let mut codes = self.opcodes();
        if codes.is_empty() {
            codes = vec![(Tag::Equal, 0, 1, 0, 1)];
        }
        if let Some(first) = codes.first_mut()
            && first.0 == Tag::Equal
        {
            let (tag, i1, i2, j1, j2) = *first;
            *first = (
                tag,
                i1.max(i2.saturating_sub(n)),
                i2,
                j1.max(j2.saturating_sub(n)),
                j2,
            );
        }
        if let Some(last) = codes.last_mut()
            && last.0 == Tag::Equal
        {
            let (tag, i1, i2, j1, j2) = *last;
            *last = (tag, i1, i2.min(i1 + n), j1, j2.min(j1 + n));
        }
        let nn = n + n;
        let mut groups = Vec::new();
        let mut group: Vec<Opcode> = Vec::new();
        for (tag, mut i1, i2, mut j1, j2) in codes {
            if tag == Tag::Equal && i2 - i1 > nn {
                group.push((tag, i1, i2.min(i1 + n), j1, j2.min(j1 + n)));
                groups.push(std::mem::take(&mut group));
                i1 = i1.max(i2.saturating_sub(n));
                j1 = j1.max(j2.saturating_sub(n));
            }
            group.push((tag, i1, i2, j1, j2));
        }
        if !group.is_empty() && !(group.len() == 1 && group[0].0 == Tag::Equal) {
            groups.push(group);
        }
        groups
    }
}

fn format_range_unified(start: usize, stop: usize) -> String {
    let mut beginning = start + 1;
    let length = stop - start;
    if length == 1 {
        return beginning.to_string();
    }
    if length == 0 {
        beginning -= 1;
    }
    format!("{beginning},{length}")
}

/// Upstream `unified_diff(a, b, fromfile, tofile, n=3, lineterm="")`.
pub fn unified_diff(
    a: &[String],
    b: &[String],
    from_file: &str,
    to_file: &str,
    n: usize,
) -> Vec<String> {
    let matcher = SequenceMatcher::new(a, b);
    let mut out = Vec::new();
    for (index, group) in matcher.grouped_opcodes(n).into_iter().enumerate() {
        if index == 0 {
            out.push(format!("--- {from_file}"));
            out.push(format!("+++ {to_file}"));
        }
        let (first, last) = (group[0], group[group.len() - 1]);
        out.push(format!(
            "@@ -{} +{} @@",
            format_range_unified(first.1, last.2),
            format_range_unified(first.3, last.4)
        ));
        for (tag, i1, i2, j1, j2) in group {
            if tag == Tag::Equal {
                out.extend(a[i1..i2].iter().map(|line| format!(" {line}")));
                continue;
            }
            if matches!(tag, Tag::Replace | Tag::Delete) {
                out.extend(a[i1..i2].iter().map(|line| format!("-{line}")));
            }
            if matches!(tag, Tag::Replace | Tag::Insert) {
                out.extend(b[j1..j2].iter().map(|line| format!("+{line}")));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.split(' ').map(str::to_string).collect()
    }

    #[test]
    fn matches_cpython_examples() {
        // difflib docs: SequenceMatcher(None, "abxcd", "abcd").get_matching_blocks()
        let a: Vec<char> = "abxcd".chars().collect();
        let b: Vec<char> = "abcd".chars().collect();
        assert_eq!(
            SequenceMatcher::new(&a, &b).matching_blocks(),
            vec![(0, 0, 2), (3, 2, 2), (5, 4, 0)]
        );
        let a: Vec<char> = "qabxcd".chars().collect();
        let b: Vec<char> = "abycdf".chars().collect();
        let tags: Vec<Tag> = SequenceMatcher::new(&a, &b)
            .opcodes()
            .iter()
            .map(|op| op.0)
            .collect();
        assert_eq!(
            tags,
            vec![
                Tag::Delete,
                Tag::Equal,
                Tag::Replace,
                Tag::Equal,
                Tag::Insert
            ]
        );
        let diff = unified_diff(
            &lines("one two three four"),
            &lines("zero one tree four"),
            "a",
            "b",
            3,
        );
        assert_eq!(
            diff,
            vec![
                "--- a",
                "+++ b",
                "@@ -1,4 +1,4 @@",
                "+zero",
                " one",
                "-two",
                "-three",
                "+tree",
                " four"
            ]
        );
        assert!(unified_diff(&lines("same"), &lines("same"), "a", "b", 3).is_empty());
    }
}
