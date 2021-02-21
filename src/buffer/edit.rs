//! Functions and structures for applying edits to a buffer.

use std::convert::TryFrom;
use std::ops::Range;

use lsp_types::TextDocumentContentChangeEvent;

use super::{Buffer, Position};

/// An edit that can be applied to a buffer.
#[derive(Debug)]
pub struct Edit {
    pub range: Range<usize>,
    pub character_range: Range<Position>,
    pub new_text: String,
}

impl Edit {
    pub fn to_text_document_content_change_event(&self) -> TextDocumentContentChangeEvent {
        TextDocumentContentChangeEvent {
            range: Some(lsp_types::Range {
                start: lsp_types::Position {
                    line: u64::try_from(self.character_range.start.y)
                        .expect("line number too large"),
                    character: u64::try_from(self.character_range.start.x)
                        .expect("character number too large"),
                },
                end: lsp_types::Position {
                    line: u64::try_from(self.character_range.end.y).expect("line number too large"),
                    character: u64::try_from(self.character_range.end.x)
                        .expect("character number too large"),
                },
            }),
            text: self.new_text.clone(),
            range_length: None,
        }
    }
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

    /// Inserts a character at the current cursor position.
    ///
    /// Returns an `Edit` representing the change.
    pub fn insert(&mut self, c: char) -> Edit {
        let start = self.byte_at_cursor();
        let start_position = Position::new(self.cursor.x(), self.cursor.y());

        let edit = Edit {
            range: start..start,
            character_range: start_position..start_position, // FIXME: naively assumes ASCII
            new_text: c.to_string(),
        };

        self.apply_edit(&edit);

        if c == '\n' {
            self.cursor
                .move_x(-isize::try_from(self.cursor.x()).expect("cursor offset too large"));
            self.cursor.move_y(1)
        } else {
            self.cursor.move_x(1);
        }

        edit
    }

    fn apply_edit(&mut self, edit: &Edit) {
        let start_position = self.storage.position_of_byte(edit.range.start);
        let old_end_position = self.storage.position_of_byte(edit.range.end);

        self.storage
            .replace_range(edit.range.start..edit.range.end, &edit.new_text);

        self.version += 1;

        let new_end_position = self
            .storage
            .position_of_byte(edit.range.start + edit.new_text.len());

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

        let old_version = buf.version;

        let edit = buf.insert('a');
        assert_eq!(buf.storage.to_string(), "a\n");

        assert_eq!(edit.new_text, "a");
        assert_eq!(edit.range.start..edit.range.end, 0..0);
        assert!(buf.version > old_version);
    }

    #[test]
    fn insert_moves_cursor() {
        let mut buf = Buffer::new();

        buf.insert('a');
        assert_eq!(buf.cursor.x(), 1);
        assert_eq!(buf.cursor.y(), 0);

        buf.insert('\n');
        assert_eq!(buf.cursor.x(), 0);
        assert_eq!(buf.cursor.y(), 1);
    }
}
