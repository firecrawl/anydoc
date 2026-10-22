//! Source coordinates retained by format frontends.

/// A zero-based row and column in a spreadsheet worksheet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpreadsheetCoordinate {
    /// Zero-based row.
    pub row: u32,
    /// Zero-based column.
    pub column: u32,
}

impl SpreadsheetCoordinate {
    /// Construct a zero-based worksheet coordinate.
    pub const fn new(row: u32, column: u32) -> Self {
        SpreadsheetCoordinate { row, column }
    }
}

/// An inclusive source range in a spreadsheet worksheet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpreadsheetRange {
    /// Inclusive range start.
    pub start: SpreadsheetCoordinate,
    /// Inclusive range end.
    pub end: SpreadsheetCoordinate,
}

impl SpreadsheetRange {
    /// Construct a range containing one worksheet cell.
    pub const fn cell(row: u32, column: u32) -> Self {
        let coordinate = SpreadsheetCoordinate::new(row, column);
        SpreadsheetRange { start: coordinate, end: coordinate }
    }

    /// Construct an inclusive range from its four zero-based bounds.
    pub const fn new(start_row: u32, start_column: u32, end_row: u32, end_column: u32) -> Self {
        SpreadsheetRange {
            start: SpreadsheetCoordinate::new(start_row, start_column),
            end: SpreadsheetCoordinate::new(end_row, end_column),
        }
    }

    /// Expand this range to include another inclusive range.
    pub fn include(&mut self, other: SpreadsheetRange) {
        self.start.row = self.start.row.min(other.start.row);
        self.start.column = self.start.column.min(other.start.column);
        self.end.row = self.end.row.max(other.end.row);
        self.end.column = self.end.column.max(other.end.column);
    }
}

/// The worksheet and source extent of a returned spreadsheet table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpreadsheetSource {
    /// Zero-based position in the source workbook's worksheet order.
    pub sheet_index: u32,
    /// Worksheet name as stored by the source format.
    pub sheet_name: String,
    /// Inclusive source extent that produced the returned table.
    pub range: SpreadsheetRange,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_are_inclusive_and_can_form_a_bounding_box() {
        let mut range = SpreadsheetRange::cell(2, 4);
        range.include(SpreadsheetRange::new(0, 1, 5, 3));
        assert_eq!(range, SpreadsheetRange::new(0, 1, 5, 4));
    }
}
