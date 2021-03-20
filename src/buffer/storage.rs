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
    use super::Storage;

    #[test]
    fn from_empty_lines() {
        let storage = Storage::from(vec![]);
        assert_eq!(storage.lines, vec![String::new()]);
    }
}
