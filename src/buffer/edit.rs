//! Functions and structures for applying edits to a buffer.

use std::ops::Range;

use super::{Buffer, Position};

/// An edit that can be applied to a buffer.
#[derive(Debug)]
pub struct Edit {
    pub range: Range<Index>,
    pub new_text: String,
}

/// A position of a specific byte within the buffer.
#[derive(Debug, Clone)]
pub struct Index {
    /// The absolute byte index within the buffer..
    pub byte: usize,

    /// The row and column containing the byte index. Columns are indexed by byte.
    pub position: Position,
}

impl Buffer {
    /// Returns the byte index of the current cursor position.
    pub fn byte_at_cursor(&self) -> usize {
        let mut byte = 0;

        for line in self.storage.iter_lines().take(self.cursor.y()) {
            byte += line.len() + 1;
        }

        byte += self.cursor.x();

        byte
    }

    fn position_of_byte(&self, byte: usize) -> Position {
        assert!(byte < self.storage.len());

        let mut remaining = byte;

        for (row, line) in self.storage.iter_lines().enumerate() {
            if remaining <= line.len() {
                return Position::new(remaining, row);
            }

            // SOMETHING WRONG WITH THIS LINE
            remaining -= line.len() - 1;
        }

        unreachable!();
    }

    /// Inserts a character at the current cursor position.
    ///
    /// Returns an `Edit` representing the change.
    pub fn insert(&mut self, c: char) -> Edit {
        let start = Index {
            byte: self.byte_at_cursor(),
            position: Position::new(self.cursor.x(), self.cursor.y()),
        };
        let end = start.clone();

        let edit = Edit {
            range: start..end,
            new_text: c.to_string(),
        };

        self.apply_edit(&edit);

        self.cursor.move_x(1);

        edit
    }

    pub fn apply_edit(&mut self, edit: &Edit) {
        let start_position = self.position_of_byte(edit.range.start.byte);
        let old_end_position = self.position_of_byte(edit.range.end.byte);

        self.storage
            .replace_range(edit.range.start.byte..edit.range.end.byte, &edit.new_text);

        let new_end_position = self.position_of_byte(edit.range.start.byte + edit.new_text.len());

        if let Some(highlighter) = &mut self.highlighter {
            highlighter.edit(edit, start_position, old_end_position, new_end_position);
        }
    }
}

#[cfg(test)]
mod tests {
    use indoc::indoc;

    use crate::buffer::{Buffer, Cursor};

    #[test]
    fn byte_at_cursor() {
        assert_eq!(Buffer::new().byte_at_cursor(), 0);

        let mut buffer = Buffer::from(indoc! {"
            Lorem ipsum
            Dolor sit amet
        "});
        buffer.cursor = Cursor::at(5, 1);

        assert_eq!(buffer.byte_at_cursor(), 17);
    }

    #[test]
    fn insert_empty_buffer() {
        let mut buf = Buffer::new();

        let edit = buf.insert('a');
        assert_eq!(buf.storage.to_string(), "a\n");

        assert_eq!(edit.new_text, "a");
        assert_eq!(edit.range.start.byte..edit.range.end.byte, 0..0);
    }
}
