//! A table style's conditional formats and which of them reach a cell.

use super::props::{PPr, RPr};
use super::table::CellProps;
use crate::wml::w_attr;
use crate::xml::{NodeId, XmlTree};

/// Conditional formats in the order Word applies them: horizontal then
/// vertical bands (Word for Mac 16.115 shows column bands over row bands),
/// first and last column, first and last row, then the corners.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Part {
    Band1Horz,
    Band2Horz,
    Band1Vert,
    Band2Vert,
    FirstCol,
    LastCol,
    FirstRow,
    LastRow,
    NeCell,
    NwCell,
    SeCell,
    SwCell,
}

impl Part {
    pub(super) const ALL: [Part; 12] = [
        Part::Band1Horz,
        Part::Band2Horz,
        Part::Band1Vert,
        Part::Band2Vert,
        Part::FirstCol,
        Part::LastCol,
        Part::FirstRow,
        Part::LastRow,
        Part::NeCell,
        Part::NwCell,
        Part::SeCell,
        Part::SwCell,
    ];

    pub(super) fn read(name: &str) -> Option<Part> {
        Some(match name {
            "band1Vert" => Part::Band1Vert,
            "band2Vert" => Part::Band2Vert,
            "band1Horz" => Part::Band1Horz,
            "band2Horz" => Part::Band2Horz,
            "firstCol" => Part::FirstCol,
            "lastCol" => Part::LastCol,
            "firstRow" => Part::FirstRow,
            "lastRow" => Part::LastRow,
            "neCell" => Part::NeCell,
            "nwCell" => Part::NwCell,
            "seCell" => Part::SeCell,
            "swCell" => Part::SwCell,
            _ => return None,
        })
    }

    fn bit(self) -> u16 {
        1 << self as u16
    }
}

/// The conditional formats that reach one cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Parts(u16);

impl Parts {
    pub(super) fn contains(self, part: Part) -> bool {
        self.0 & part.bit() != 0
    }

    fn with(self, part: Part, on: bool) -> Parts {
        if on { Parts(self.0 | part.bit()) } else { self }
    }

    /// In the order they apply.
    pub(super) fn iter(self) -> impl Iterator<Item = Part> {
        Part::ALL
            .into_iter()
            .filter(move |part| self.contains(*part))
    }
}

/// What a conditional format sets.
#[derive(Debug, Clone, Default)]
pub(super) struct PartLook {
    pub(super) ppr: PPr,
    pub(super) rpr: RPr,
    pub(super) cell: CellProps,
}

impl PartLook {
    pub(super) fn merge(&mut self, other: &PartLook) {
        self.ppr.merge(&other.ppr);
        self.rpr.merge(&other.rpr);
        self.cell.merge(&other.cell);
    }
}

/// `w:tblLook`: which conditional formats the table shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TableLook {
    first_row: bool,
    last_row: bool,
    first_column: bool,
    last_column: bool,
    row_bands: bool,
    column_bands: bool,
}

/// A table without `w:tblLook` shows what Word's new tables do (`04A0`).
const DEFAULT_LOOK: u32 = 0x04A0;

impl TableLook {
    /// The attributes, else the `w:val` bitmask. Word for Mac 16.115 reads
    /// no bitmask once any attribute is there, and takes a missing one as on.
    pub(super) fn read(tree: &XmlTree, look: Option<NodeId>) -> TableLook {
        const NAMES: [&str; 6] = [
            "firstRow",
            "lastRow",
            "firstColumn",
            "lastColumn",
            "noHBand",
            "noVBand",
        ];
        let attributes =
            look.is_some_and(|look| NAMES.iter().any(|name| w_attr(tree, look, name).is_some()));
        let mask = look.map_or(Some(DEFAULT_LOOK), |look| {
            w_attr(tree, look, "val").and_then(|v| u32::from_str_radix(v, 16).ok())
        });
        let flag = |name: &str, bit: u32| match look.filter(|_| attributes) {
            Some(look) => w_attr(tree, look, name).is_none_or(|v| matches!(v, "1" | "true" | "on")),
            None => mask.is_some_and(|mask| mask & bit != 0),
        };
        TableLook {
            first_row: flag("firstRow", 0x0020),
            last_row: flag("lastRow", 0x0040),
            first_column: flag("firstColumn", 0x0080),
            last_column: flag("lastColumn", 0x0100),
            row_bands: !flag("noHBand", 0x0200),
            column_bands: !flag("noVBand", 0x0400),
        }
    }

    #[cfg(feature = "pdf")]
    pub(crate) fn first_row(self) -> bool {
        self.first_row
    }
}

/// A cell's place in its table, counted in cells.
#[derive(Debug, Clone, Copy)]
pub(super) struct Position {
    pub(super) row: usize,
    pub(super) rows: usize,
    pub(super) column: usize,
    pub(super) columns: usize,
}

/// The cells a conditional format covers, inclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Region {
    pub(super) top: usize,
    pub(super) bottom: usize,
    pub(super) left: usize,
    pub(super) right: usize,
}

/// Band sizes in rows and columns (`w:tblStyleRowBandSize`,
/// `w:tblStyleColBandSize`), at least 1.
pub(super) type Bands = (usize, usize);

impl TableLook {
    /// The formats that reach the cell at `at`, each with the cells it
    /// covers; bands skip a shown first and last row or column.
    pub(super) fn parts(self, at: Position, bands: Bands) -> Vec<(Part, Region)> {
        let last_row = at.rows.saturating_sub(1);
        let last_column = at.columns.saturating_sub(1);
        let first_row = self.first_row && at.row == 0;
        let last_row_on = self.last_row && at.row == last_row;
        let first_column = self.first_column && at.column == 0;
        let last_column_on = self.last_column && at.column == last_column;
        let rows = Region {
            top: 0,
            bottom: last_row,
            left: 0,
            right: last_column,
        };
        let mut out = Vec::new();
        let band = |index: usize, skip_first: bool, skip_last: bool, last: usize, size: usize| {
            let from = usize::from(skip_first);
            let to = if skip_last {
                last.checked_sub(1)?
            } else {
                last
            };
            if index < from || index > to {
                return None;
            }
            let start = from + (index - from) / size * size;
            Some((
                ((index - from) / size).is_multiple_of(2),
                start,
                (start + size - 1).min(to),
            ))
        };
        if self.row_bands
            && let Some((odd, top, bottom)) =
                band(at.row, self.first_row, self.last_row, last_row, bands.0)
        {
            let part = if odd {
                Part::Band1Horz
            } else {
                Part::Band2Horz
            };
            out.push((
                part,
                Region {
                    top,
                    bottom,
                    ..rows
                },
            ));
        }
        if self.column_bands
            && let Some((odd, left, right)) = band(
                at.column,
                self.first_column,
                self.last_column,
                last_column,
                bands.1,
            )
        {
            let part = if odd {
                Part::Band1Vert
            } else {
                Part::Band2Vert
            };
            out.push((
                part,
                Region {
                    left,
                    right,
                    ..rows
                },
            ));
        }
        let column = |index| Region {
            left: index,
            right: index,
            ..rows
        };
        let row = |index| Region {
            top: index,
            bottom: index,
            ..rows
        };
        let cell = Region {
            top: at.row,
            bottom: at.row,
            left: at.column,
            right: at.column,
        };
        let candidates = [
            (Part::FirstCol, first_column, column(0)),
            (Part::LastCol, last_column_on, column(last_column)),
            (Part::FirstRow, first_row, row(0)),
            (Part::LastRow, last_row_on, row(last_row)),
            (Part::NeCell, first_row && last_column_on, cell),
            (Part::NwCell, first_row && first_column, cell),
            (Part::SeCell, last_row_on && last_column_on, cell),
            (Part::SwCell, last_row_on && first_column, cell),
        ];
        out.extend(
            candidates
                .into_iter()
                .filter(|(_, on, _)| *on)
                .map(|(part, _, region)| (part, region)),
        );
        out
    }
}

/// The parts of `regions`.
pub(super) fn parts_of(regions: &[(Part, Region)]) -> Parts {
    regions
        .iter()
        .fold(Parts::default(), |parts, (part, _)| parts.with(*part, true))
}

impl Parts {
    /// The first row's formats, for a scan that does not place cells.
    #[cfg(feature = "pdf")]
    pub(crate) fn first_row(on: bool) -> Parts {
        Parts::default().with(Part::FirstRow, on)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xml::parse_str;

    fn look(attributes: &str) -> TableLook {
        let tree = parse_str(&format!(
            "<w:tblLook xmlns:w=\"{}\" {attributes}/>",
            crate::wml::W
        ))
        .unwrap();
        TableLook::read(&tree, Some(tree.root()))
    }

    fn parts(look: TableLook, row: usize, column: usize) -> Vec<Part> {
        let at = Position {
            row,
            rows: 6,
            column,
            columns: 4,
        };
        look.parts(at, (1, 1))
            .into_iter()
            .map(|(part, _)| part)
            .collect()
    }

    #[test]
    fn any_attribute_sets_the_look_and_a_missing_one_is_on() {
        let mask = look(r#"w:val="04A0""#);
        assert!(mask.first_row && mask.first_column && mask.row_bands);
        assert!(!mask.last_row && !mask.last_column && !mask.column_bands);
        // Word's cascade.docx table 3: the bitmask is ignored, and every
        // attribute left out reads as 1, noHBand and noVBand included.
        let table_3 = look(r#"w:val="04A0" w:firstColumn="0""#);
        assert_eq!(
            table_3,
            TableLook {
                first_row: true,
                last_row: true,
                first_column: false,
                last_column: true,
                row_bands: false,
                column_bands: false,
            }
        );
        let all = look(
            r#"w:firstRow="1" w:lastRow="0" w:firstColumn="1" w:lastColumn="0" w:noHBand="0" w:noVBand="1" w:val="0000""#,
        );
        assert!(all.first_row && all.first_column && all.row_bands);
        assert!(!all.last_row && !all.last_column && !all.column_bands);
        assert_eq!(look(""), look(r#"w:val="0000""#));
        assert!(look("").row_bands && look("").column_bands);
    }

    #[test]
    fn bands_count_from_the_first_body_row_and_column() {
        let all = look(r#"w:val="01E0""#);
        use Part::*;
        assert_eq!(parts(all, 0, 0), [FirstCol, FirstRow, NwCell]);
        assert_eq!(
            parts(all, 1, 1),
            [Band1Horz, Band1Vert],
            "column bands apply last"
        );
        assert_eq!(parts(all, 2, 2), [Band2Horz, Band2Vert]);
        assert_eq!(parts(all, 5, 3), [LastCol, LastRow, SeCell]);
        assert_eq!(parts(all, 0, 3), [LastCol, FirstRow, NeCell]);
        let plain = look(r#"w:val="0600""#);
        assert!(parts(plain, 1, 1).is_empty(), "no bands, no first row");
    }

    #[test]
    fn a_band_of_two_rows_covers_both() {
        let at = |row| Position {
            row,
            rows: 7,
            column: 0,
            columns: 1,
        };
        let look = look(r#"w:val="0420""#);
        let bands: Vec<_> = (1..7)
            .map(|row| look.parts(at(row), (2, 1)).into_iter().next().unwrap())
            .collect();
        assert_eq!(
            bands[0],
            (
                Part::Band1Horz,
                Region {
                    top: 1,
                    bottom: 2,
                    left: 0,
                    right: 0
                }
            )
        );
        assert_eq!(bands[1].1, bands[0].1);
        assert_eq!(bands[2].0, Part::Band2Horz);
        assert_eq!(
            bands[5],
            (
                Part::Band1Horz,
                Region {
                    top: 5,
                    bottom: 6,
                    left: 0,
                    right: 0
                }
            )
        );
    }
}
