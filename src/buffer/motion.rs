//! Cursor motions within a buffer.

use std::cmp;
use std::convert::TryFrom;

use euclid::vec2;

use super::{Buffer, Offset, Position};

/// The amount of padding that the cursor will maintain opposite the viewport.
const SCROLLOFF: usize = 5;

/// A cursor for an individual buffer.
#[derive(Debug, Default, Copy, Clone)]
pub struct Cursor {
    /// Position of the cursor.
    pos: Position,

    /// The column that the cursor should snap to if possible.
    ///
    /// See the `snap` method for more detail.
    desired_col: usize,
}

impl Cursor {
    pub fn x(&self) -> usize {
        self.pos.x
    }

    pub fn y(&self) -> usize {
        self.pos.y
    }

    /// Move the cursor up or down a number of lines.
    pub fn move_y(&mut self, offset: isize) {
        let n = usize::try_from(offset.abs()).expect("expected non-negative offset");

        if offset.is_negative() {
            self.pos.y -= n;
        } else {
            self.pos.y += n;
        }
    }

    /// "Snaps" the cursor to be within a given distance.
    ///
    /// This method is used to support vim-like end-of-line behavior. If the cursor is moved
    /// vertically to a line of text that is shorter than the current X-coordinate, the cursor
    /// moves left to be within the text. The cursor remembers the desired line length. If the
    /// cursor is moved back to a line that is longer, it will be snapped back as close to the
    /// desired coordinate as possible, even if the line is still too short.
    pub fn snap(&mut self, line_length: usize) {
        if self.desired_col != self.pos.x {
            self.pos.x = cmp::min(self.desired_col, line_length)
        } else if self.pos.x > line_length {
            self.pos.x = line_length
        }
    }

    #[cfg(test)]
    /// Creates a cursor at a particular position.
    pub fn at(x: usize, y: usize) -> Cursor {
        Cursor {
            pos: Position::new(x, y),
            desired_col: x,
        }
    }
}

impl Buffer {
    pub fn move_offset(&mut self, offset: Offset) {
        let (_, y_offset) = offset.to_tuple();

        self.cursor.move_y(y_offset);
        self.cursor.snap(self.current_line_length());

        if let Some(viewport) = &mut self.viewport {
            if self.cursor.y() > SCROLLOFF && self.cursor.y() > viewport.max_y() - SCROLLOFF {
                let max_y = cmp::min(self.cursor.y() + SCROLLOFF, self.lines.len());
                viewport.origin.y = max_y - viewport.height();
            } else if self.cursor.y() < viewport.min_y() + SCROLLOFF {
                viewport.origin.y = self.cursor.y().saturating_sub(SCROLLOFF);
            }
        }
    }

    /// Move the cursor down a single line.
    pub fn move_down(&mut self) {
        if self.at_last_line() {
            return;
        }

        self.move_offset(vec2(0, 1));
    }

    /// Move the cursor up a single line.
    pub fn move_up(&mut self) {
        if self.at_first_line() {
            return;
        }

        self.move_offset(vec2(0, -1));
    }

    /// Returns true if the cursor is on the first line of the buffer.
    fn at_first_line(&self) -> bool {
        self.cursor.y() == 0
    }

    /// Returns true if the cursor is on the last line of the buffer.
    fn at_last_line(&self) -> bool {
        self.cursor.y() == self.lines.len() - 1
    }

    /// Returns the current line length in cells.
    fn current_line_length(&self) -> usize {
        let current_line = &self.lines[self.cursor.y()];
        // FIXME: Naively assumes ASCII.
        current_line.len()
    }
}

#[cfg(test)]
mod tests {
    use super::Buffer;

    use euclid::{rect, size2};
    use indoc::indoc;
    use itertools::Itertools;

    use crate::buffer::{Cursor, Position, Span};

    #[test]
    fn move_single_character_empty_buffer() {
        let mut buffer = Buffer::new();

        buffer.move_down();
        assert_eq!(buffer.cursor.pos, Position::default());

        buffer.move_up();
        assert_eq!(buffer.cursor.pos, Position::default());
    }

    #[test]
    fn move_up() {
        let mut buffer = Buffer::from(indoc! {"
            foo
            bar
        "});
        buffer.cursor = Cursor::at(0, 1);

        buffer.move_up();

        assert_eq!(buffer.cursor.pos, Position::new(0, 0));
    }

    #[test]
    fn move_down() {
        let mut buffer = Buffer::from(indoc! {"
            foo
            bar
        "});
        buffer.cursor = Cursor::at(2, 0);

        buffer.move_down();

        assert_eq!(buffer.cursor.pos, Position::new(2, 1));
    }

    #[test]
    fn move_down_out_of_bounds() {
        let mut buffer = Buffer::from(indoc! {"
            abcdef
            a
            abc
            abcdef
        "});

        buffer.cursor = Cursor::at(5, 0);

        buffer.move_down();
        assert_eq!(buffer.cursor.pos, Position::new(1, 1));

        buffer.move_down();
        assert_eq!(buffer.cursor.pos, Position::new(3, 2));

        buffer.move_down();
        assert_eq!(buffer.cursor.pos, Position::new(5, 3));
    }

    #[test]
    fn move_up_out_of_bounds() {
        let mut buffer = Buffer::from(indoc! {"
            abcdef
            a
            abc
            abcdef
        "});

        buffer.cursor = Cursor::at(5, 3);

        buffer.move_up();
        assert_eq!(buffer.cursor.pos, Position::new(3, 2));

        buffer.move_up();
        assert_eq!(buffer.cursor.pos, Position::new(1, 1));

        buffer.move_up();
        assert_eq!(buffer.cursor.pos, Position::new(5, 0));
    }

    #[test]
    fn viewport_motion_up() {
        let mut buffer = Buffer::from((1..100).join("\n").as_str());

        buffer.viewport = Some(rect(0, 20, 10, 60));
        buffer.cursor = Cursor::at(0, 50);

        let old_viewport = buffer.viewport.unwrap();
        buffer.move_up();
        assert_eq!(buffer.cursor.pos, Position::new(0, 49));
        assert_eq!(old_viewport, buffer.viewport.unwrap());

        for _ in 0..49 {
            buffer.move_up();
        }

        assert_eq!(buffer.cursor.pos, Position::zero());
        assert_eq!(buffer.viewport.unwrap().height(), old_viewport.height());
        assert_eq!(buffer.viewport.unwrap().min_y(), 0);
    }

    #[test]
    fn viewport_motion_down() {
        let mut buffer = Buffer::from((1..100).join("\n").as_str());

        buffer.viewport = Some(rect(0, 20, 10, 60));
        buffer.cursor = Cursor::at(0, 50);

        let old_viewport = buffer.viewport.unwrap();
        buffer.move_down();
        assert_eq!(buffer.cursor.pos, Position::new(0, 51));
        assert_eq!(old_viewport, buffer.viewport.unwrap());

        for _ in 0..49 {
            buffer.move_down();
        }

        assert_eq!(buffer.cursor.pos, Position::new(0, 98));
        assert_eq!(buffer.viewport.unwrap().height(), old_viewport.height());
        assert_eq!(buffer.viewport.unwrap().max_y(), 99);
    }

    #[test]
    fn scrolloff_larger_than_viewport() {
        let mut buffer = Buffer::from(indoc! {"
            12
            34
        "});

        buffer.viewport = Some(Span::from_size(size2(2, 2)));

        buffer.move_down();
        assert_eq!(buffer.viewport.unwrap().origin, Position::zero());
        buffer.move_up();
        assert_eq!(buffer.viewport.unwrap().origin, Position::zero());
    }
}
