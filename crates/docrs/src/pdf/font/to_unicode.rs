//! The `ToUnicode` CMap: each CID to the text it was drawn for.

use super::Cid;
use crate::pdf::writer::write_hex;

/// ISO 32000-1 9.10.3 caps a `beginbfchar` block at 100 entries.
const BLOCK: usize = 100;

const HEAD: &str = "/CIDInit /ProcSet findresource begin\n\
12 dict begin\n\
begincmap\n\
/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n\
/CMapName /Adobe-Identity-UCS def\n\
/CMapType 2 def\n\
1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n";

const TAIL: &str = "endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n";

/// Entries in CID order; empty texts are left out. Text is UTF-16BE, so a
/// supplementary character is a surrogate pair and a cluster maps to all
/// of its characters.
pub(super) fn cmap<'a>(entries: impl Iterator<Item = (Cid, &'a str)>) -> Vec<u8> {
    let entries: Vec<(Cid, &str)> = entries.filter(|(_, text)| !text.is_empty()).collect();
    let mut out = HEAD.as_bytes().to_vec();
    for block in entries.chunks(BLOCK) {
        out.extend_from_slice(format!("{} beginbfchar\n", block.len()).as_bytes());
        for (cid, text) in block {
            write_hex(&mut out, &cid.to_be_bytes());
            out.push(b' ');
            let utf16: Vec<u8> = text.encode_utf16().flat_map(u16::to_be_bytes).collect();
            write_hex(&mut out, &utf16);
            out.push(b'\n');
        }
        out.extend_from_slice(b"endbfchar\n");
    }
    out.extend_from_slice(TAIL.as_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surrogate_pairs_and_clusters() {
        let map = cmap([(1, "A"), (2, ""), (3, "\u{1F130}"), (4, "fi")].into_iter());
        let map = String::from_utf8(map).unwrap();
        assert!(map.contains(
            "3 beginbfchar\n<0001> <0041>\n<0003> <D83CDD30>\n<0004> <00660069>\nendbfchar\n"
        ));
        assert!(!map.contains("<0002>"));
    }

    #[test]
    fn blocks_hold_at_most_100_entries() {
        let texts: Vec<String> = (0..250)
            .map(|n| char::from(b'a' + (n % 26) as u8).to_string())
            .collect();
        let map = cmap(
            texts
                .iter()
                .enumerate()
                .map(|(cid, text)| (cid as Cid + 1, text.as_str())),
        );
        let map = String::from_utf8(map).unwrap();
        let blocks: Vec<&str> = map.matches(" beginbfchar").collect();
        assert_eq!(blocks.len(), 3);
        assert!(map.contains("100 beginbfchar") && map.contains("50 beginbfchar"));
    }
}
