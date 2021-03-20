use std::cmp;
use std::fmt;
use std::ops::{Index, Range};

use super::Position;

/// Underlying storage for the buffer contents.
///
/// The storage contains at least one (empty) line.
#[derive(Debug, PartialEq, Eq)]
pub struct Storage {
    /// The contents of the storage.
    ///
    /// Unix-style newlines ("\n") are implicitly inserted between each line. Lines themselves
    /// cannot contain `\n`.
    lines: Vec<String>,
}

impl Storage {
    /// Returns a new `Storage` with a single empty line.
    pub fn new() -> Self {
        Self {
            lines: vec![String::new()],
        }
    }

    /// Returns the number of lines.
    pub fn lines(&self) -> usize {
        self.lines.len()
    }

    /// Returns the total byte length of the buffer.
    pub fn len(&self) -> usize {
        let mut len = 0;

        for line in &self.lines {
            len += line.len() + 1
        }

        len
    }

    /// Returns the width of a given line.
    pub fn line_width(&self, line: usize) -> usize {
        // FIXME: Naively assumes ASCII
        self.lines[line].len()
    }

    /// Returns an iterator over the lines of the storage.
    pub fn iter_lines(&self) -> impl Iterator<Item = &str> {
        self.lines.iter().map(|line| &**line)
    }

    /// Return a slice of the underlying text starting at the given position.
    ///
    /// The slice returned may be of any length.
    pub fn slice_at(&self, pos: Position) -> impl AsRef<[u8]> + '_ {
        if pos.y == self.lines() {
            return "";
        }

        let line = &self.lines[pos.y];

        if pos.x == line.len() {
            "\n"
        } else {
            &line[pos.x..]
        }
    }

    /// Replace a byte range in the buffer with a replacement string, like
    /// [`String::replace_range`].
    pub fn replace_range(&mut self, range: Range<usize>, replacement: &str) {
        // Find the line containing the start of the byte range, and the byte offset from the
        // start of the line.
        let mut line_no = 0;
        let mut byte_offset = range.start;
        while byte_offset > self.lines[line_no].len() {
            byte_offset -= self.lines[line_no].len() + 1;
            line_no += 1;
        }

        // Delete any text that is inside the range.
        let mut bytes_to_consume = range.len();

        while bytes_to_consume > 0 {
            let bytes_to_remove =
                cmp::min(self.lines[line_no][byte_offset..].len(), bytes_to_consume);
            self.lines[line_no].replace_range(byte_offset..(byte_offset + bytes_to_remove), "");

            bytes_to_consume -= bytes_to_remove;

            if bytes_to_consume > 0 {
                // Remove the newline.
                let next_line = self.lines.remove(line_no + 1);
                self.lines[line_no].insert_str(byte_offset, &next_line);
                bytes_to_consume -= 1;
            }
        }

        // Insert the new text.
        if !replacement.contains('\n') {
            // Fast path. Just insert the new text into the current line.
            self.lines[line_no].insert_str(byte_offset, replacement);
        } else {
            // We're going to add at least one new line into the underlying lines array. Start by
            // splitting the current line into two at the insertion point.
            let end = self.lines[line_no].split_off(byte_offset);
            self.lines.insert(line_no + 1, end);

            let mut new_lines = replacement.lines().peekable();

            // The first new line is appended at the insertion point.
            let first_new_line = new_lines
                .next()
                .expect("checked replacement text contains newline above");
            self.lines[line_no].push_str(first_new_line);

            while let Some(new_line) = new_lines.next() {
                line_no += 1;

                if new_lines.peek().is_some() {
                    // Middle new lines, if any, are inserted as their own lines.
                    self.lines.insert(line_no, new_line.to_owned());
                } else {
                    // The last new line is prepended to line split after the insertion point.
                    self.lines[line_no].insert_str(0, new_line);
                }
            }
        }
    }
}

impl From<Vec<String>> for Storage {
    fn from(lines: Vec<String>) -> Self {
        Self {
            lines: if lines.is_empty() {
                vec![String::new()]
            } else {
                lines
            },
        }
    }
}

impl<'a> From<&'a str> for Storage {
    fn from(s: &str) -> Self {
        Self {
            lines: s.lines().map(|line| line.to_owned()).collect(),
        }
    }
}

impl fmt::Display for Storage {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        for line in &self.lines {
            writeln!(f, "{}", line)?;
        }

        Ok(())
    }
}

impl Index<Range<Position>> for Storage {
    type Output = str;

    fn index(&self, Range { start, end }: Range<Position>) -> &Self::Output {
        assert!(
            start.y == end.y,
            "cannot index across rows: {:?}",
            start..end
        );
        &self.lines[start.y][start.x..end.x]
    }
}

#[cfg(test)]
mod tests {
    use indoc::indoc;

    use super::Storage;

    #[test]
    fn from_empty_lines() {
        let storage = Storage::from(vec![]);
        assert_eq!(storage.lines, vec![String::new()]);
    }

    #[test]
    fn replace_range_deletion() {
        let mut storage = Storage::from("Goodbye, cruel world!");

        storage.replace_range(8..14, "");

        assert_eq!(storage.to_string(), "Goodbye, world!\n");
    }

    #[test]
    fn replace_range_middle() {
        let mut storage = Storage::from(indoc! {"
            a b c
            one three
        "});

        storage.replace_range(10..10, "two ");

        assert_eq!(storage.to_string(), "a b c\none two three\n");
    }

    #[test]
    fn replace_range_delete_newline() {
        let mut storage = Storage::from("this is not \none line");

        storage.replace_range(8..13, "");

        assert_eq!(storage.to_string(), "this is one line\n");
    }

    #[test]
    fn replace_range_replacement_contains_newlines() {
        let mut storage = Storage::from("ae");

        storage.replace_range(1..1, "b\nc\nd");

        assert_eq!(storage.to_string(), "ab\nc\nde\n");
    }

    #[test]
    fn replace_range_at_end_of_line() {
        let mut storage = Storage::from("a\n");

        storage.replace_range(1..1, "b");

        assert_eq!(storage.to_string(), "ab\n");
    }
}
