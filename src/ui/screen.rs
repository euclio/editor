use std::fmt::{self, Debug, Write};
use std::ops::{Index, IndexMut};

use itertools::Itertools;

use super::{Bounds, Color, Coordinates, Size};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    pub c: char,
    pub color: Option<Color>,
}

impl Default for Cell {
    fn default() -> Self {
        Cell {
            c: ' ',
            color: None,
        }
    }
}

impl From<char> for Cell {
    fn from(c: char) -> Self {
        Cell { c, color: None }
    }
}

#[derive(Default)]
pub struct Screen {
    pub size: Size,
    cells: Vec<Cell>,
}

impl Screen {
    pub fn new(size: Size) -> Self {
        Screen {
            size,
            cells: vec![Cell::default(); (size.width * size.height).into()],
        }
    }

    pub fn iter_rows(&self) -> impl Iterator<Item = impl Iterator<Item = &Cell>> {
        (0..usize::from(self.size.height)).map(move |row| {
            let width = usize::from(self.size.width);
            let row_start = row * width;
            self.cells[row_start..row_start + width].iter()
        })
    }

    /// Convenience method to write a string starting at a specific coordinate. If the string is
    /// longer than the width of the screen, it is truncated.
    pub fn write(&mut self, Coordinates { y, x, .. }: Coordinates, text: &str) {
        for (i, c) in text
            .chars()
            .enumerate()
            .take(usize::from(self.size.width - x))
        {
            self[(y, x + i as u16)].c = c;
        }
    }

    /// Apply a color to cells within a rectangular region.
    pub fn apply_color(&mut self, bounds: Bounds, color: Color) {
        // FIXME: I think there's an off-by-one here if the highlight is on the last line.
        for y in bounds.min.y..=bounds.max.y {
            for x in bounds.min.x..bounds.max.x {
                self[(y, x)].color = Some(color);
            }
        }
    }

    /// Returns the index in the underlying storage that corresponds to the given row and column.
    ///
    /// # Panics
    ///
    /// Panics if the row or column are out of bounds.
    fn idx(&self, (row, col): (u16, u16)) -> usize {
        assert!(
            row < self.size.height,
            "there are {} rows but the row is {}",
            self.size.height,
            row
        );
        assert!(
            col < self.size.width,
            "there are {} columns but the column is {}",
            self.size.width,
            col
        );

        usize::from(row * self.size.width + col)
    }
}

impl Index<(u16, u16)> for Screen {
    type Output = Cell;

    fn index(&self, (row, col): (u16, u16)) -> &Self::Output {
        &self.cells[self.idx((row, col))]
    }
}

impl IndexMut<(u16, u16)> for Screen {
    fn index_mut(&mut self, (row, col): (u16, u16)) -> &mut Self::Output {
        let idx = self.idx((row, col));
        &mut self.cells[idx]
    }
}

impl Debug for Screen {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        for row in self.iter_rows() {
            f.write_str(&row.into_iter().map(|cell| format!("{:?}", cell)).join(", "))?;
            f.write_char('\n')?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Bounds, Cell, Color, Coordinates, Screen, Size};

    #[test]
    fn indexing() {
        let mut buf = Screen::new(Size::new(3, 3));
        buf[(0, 0)] = Cell::from('a');
        buf[(2, 2)] = Cell::from('z');

        assert_eq!(buf.cells[0], Cell::from('a'));
        assert_eq!(buf.cells[8], Cell::from('z'));
    }

    #[test]
    #[should_panic = "there are 10 rows"]
    fn indexing_out_of_bounds_row() {
        let buf = Screen::new(Size::new(10, 10));
        let _ = &buf[(11, 0)];
    }

    #[test]
    #[should_panic = "there are 3 columns"]
    fn indexing_out_of_bounds_col() {
        let buf = Screen::new(Size::new(3, 3));
        let _ = &buf[(0, 3)];
    }

    #[test]
    fn iter_rows() {
        let mut buf = Screen::new(Size::new(3, 3));
        buf[(0, 0)] = Cell::from('a');

        let rows = buf
            .iter_rows()
            .map(|row| row.cloned().collect::<Vec<_>>())
            .collect::<Vec<_>>();

        assert_eq!(
            rows,
            vec![
                vec![Cell::from('a'), Cell::default(), Cell::default()],
                vec![Cell::default(), Cell::default(), Cell::default()],
                vec![Cell::default(), Cell::default(), Cell::default()],
            ]
        );
    }

    #[test]
    fn write_too_long() {
        let mut buf = Screen::new(Size::new(2, 1));
        buf.write(Coordinates::new(0, 0), "hello, world");

        println!("{:#?}", buf);

        assert_eq!(
            buf.iter_rows().next().unwrap().collect::<Vec<_>>(),
            vec![&Cell::from('h'), &Cell::from('e')],
        );
    }

    #[test]
    fn apply_color() {
        let mut buf = Screen::new(Size::new(5, 5));
        let bounds = Bounds::new(Coordinates::new(1, 1), Coordinates::new(2, 1));
        buf.apply_color(bounds, Color::BLUE);

        assert_eq!(buf[(0, 0)].color, None);
        assert_eq!(buf[(1, 1)].color, Some(Color::BLUE));
        assert_eq!(buf[(1, 2)].color, None);
    }
}
