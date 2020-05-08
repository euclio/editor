//! Traits and types for drawing to an abstracted screen.

use euclid::{Box2D, Point2D, Size2D};

/// Used to group units that deal with the screen.
pub struct ScreenSpace;

/// The XY coordinates of a cell on the screen, starting from (0, 0) at the top left. The
/// Y-coordinate is the row, and the X-coordinate is the column.
pub type Coordinates = Point2D<u16, ScreenSpace>;

/// A width and height on the screen, in cells.
pub type Size = Size2D<u16, ScreenSpace>;

/// A bounding rectangle on the screen, in cells.
pub type Bounds = Box2D<u16, ScreenSpace>;

mod screen;

pub use screen::Screen;

/// Context for the rendering of a widget.
pub struct Context<'screen> {
    /// The bounds that the widget should be drawn within.
    ///
    /// It is the `Drawable` implementation's responsibility to *not* draw outside these bounds.
    pub bounds: Bounds,

    pub screen: &'screen mut Screen,
}

/// Objects that can draw themselves to a screen.
pub trait Drawable {
    fn draw(&self, ctx: &mut Context);
}
