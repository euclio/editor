//! Functions and structures for applying edits to a buffer.

use std::convert::TryFrom;
use std::ops::Range;

use lsp_types::TextDocumentContentChangeEvent;

use crate::buffer::units::{ByteIndex, CharPosition};

use super::Buffer;

/// An edit that can be applied to a buffer.
#[derive(Debug)]
pub struct Edit {
    pub range: Range<ByteIndex>,
    pub character_range: Range<CharPosition>,
    pub new_text: String,
}

impl Edit {
    /// Returns the index of the end of the replaced text.
    pub fn new_end(&self) -> ByteIndex {
        self.range.start + ByteIndex::new(self.new_text.len())
    }

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
    fn byte_at_cursor(&self) -> ByteIndex {
        let mut byte = 0;

        for line in self.storage.iter_lines().take(self.cursor.y()) {
            byte += line.len() + 1;
        }

        byte += self.cursor.x();

        ByteIndex::new(byte)
    }

    /// Inserts a character at the current cursor position.
    ///
    /// Returns an `Edit` representing the change.
    pub fn insert(&mut self, c: char) -> Edit {
        let byte = self.byte_at_cursor();
        let edit = self.edit(byte..byte, c.to_string());

        let pos = self.storage.position_of_byte(edit.new_end());
        self.cursor.set_x(pos.x);
        self.cursor.set_y(pos.y);

        edit
    }

    /// Delete the character immediately preceding the cursor.
    pub fn delete(&mut self) -> Option<Edit> {
        let end = self.byte_at_cursor();

        if end == ByteIndex::new(0) {
            return None;
        }

        // FIXME: Naively assumes ASCII
        let start = end - ByteIndex::new(1);
        let edit = self.edit(start..end, String::new());

        let pos = self.storage.position_of_byte(start);
        self.cursor.set_x(pos.x);
        self.cursor.set_y(pos.y);

        Some(edit)
    }

    /// Replaces a byte range in the storage with a new string, and constructs an `Edit` that
    /// represents that change.
    ///
    /// - The buffer's version is incremented.
    /// - The buffer's highlighter is notified of the edit.
    fn edit(&mut self, range: Range<ByteIndex>, new_text: String) -> Edit {
        let start_position = self.storage.position_of_byte(range.start);
        let old_end_position = self.storage.position_of_byte(range.end);

        let character_range = self.storage.byte_to_char_position(range.start)
            ..self.storage.byte_to_char_position(range.end);

        self.storage
            .replace_range(range.start.0..range.end.0, &new_text);
        self.version += 1;

        let new_end_position = self
            .storage
            .position_of_byte(range.start + ByteIndex::new(new_text.len()));

        let edit = Edit {
            range,
            character_range,
            new_text,
        };

        if let Some(highlighter) = &mut self.highlighter {
            highlighter.edit(&edit, start_position, old_end_position, new_end_position);
        }

        edit
    }
}

#[cfg(test)]
mod tests {
    use indoc::indoc;

    use crate::buffer::{Buffer, Cursor};

    use super::ByteIndex;

    #[test]
    fn byte_at_cursor() {
        assert_eq!(Buffer::new().byte_at_cursor(), ByteIndex::new(0));

        let mut buffer = Buffer::from(indoc! {"
            Lorem ipsum
            Dolor sit amet
        "});
        buffer.cursor = Cursor::at(5, 1);

        assert_eq!(buffer.byte_at_cursor(), ByteIndex::new(17));
    }

    #[test]
    fn insert_empty_buffer() {
        let mut buf = Buffer::new();

        let old_version = buf.version;

        let edit = buf.insert('a');
        assert_eq!(buf.storage.to_string(), "a\n");

        assert_eq!(edit.new_text, "a");
        assert_eq!(edit.range.start, ByteIndex::new(0));
        assert_eq!(edit.range.end, ByteIndex::new(0));
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

    #[test]
    fn delete_at_middle_of_line() {
        let mut buf = Buffer::from("abc");
        buf.cursor.set_x(2);

        buf.delete();

        assert_eq!(buf.storage.to_string(), "ac\n");
        assert_eq!(buf.cursor.x(), 1);
        assert_eq!(buf.cursor.y(), 0);
    }

    #[test]
    fn delete_beginning_of_line() {
        let mut buf = Buffer::from("a\nb");
        buf.cursor.set_y(1);
        buf.cursor.set_x(0);

        buf.delete();

        assert_eq!(buf.storage.to_string(), "ab\n");
        assert_eq!(buf.cursor.x(), 1);
        assert_eq!(buf.cursor.y(), 0);
    }

    #[test]
    fn delete_beginning_of_buffer() {
        let mut buf = Buffer::new();

        let edit = buf.delete();

        assert!(edit.is_none());
    }
}
