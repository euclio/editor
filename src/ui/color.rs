use std::fmt::{self, Debug};

#[derive(Copy, Clone, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Color { r, g, b }
    }

    pub const BLUE: Color = Color::new(0, 0, 0xFF);
}

impl Debug for Color {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

#[cfg(test)]
mod tests {
    use super::Color;

    #[test]
    fn debug() {
        assert_eq!(format!("{:?}", Color::new(0xAB, 0xCD, 0xEF)), "#abcdef");
        assert_eq!(format!("{:?}", Color::new(0x00, 0x00, 0x00)), "#000000");
    }
}
