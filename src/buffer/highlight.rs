//! Syntax highlighting for buffer contents via [tree-sitter].
//!
//! # Notes about `Range`
//!
//! This module uses tree-sitter's convenient `Range` type for representing regions of the buffer,
//! but it differs slightly from the other geometric types used by the editor:
//!
//! - Ranges are endpoint-exclusive only on the X-axis. So, we have to add 1 to
//! the Y-coordinate to get non-empty areas for single-line ranges.
//! - Ranges work like a highlighter. Multi-line ranges implicitly include all text between the
//! points. Therefore nanges cannot be converted directly to rectangular buffer `Span`s.
//!
//! [tree-sitter]: https://tree-sitter.github.io/tree-sitter/

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Index;

use lazy_static::lazy_static;
use log::*;
use maplit::hashmap;
use tree_sitter::{Parser, Point, Query, QueryCursor, Range, Tree};

use crate::syntax::Syntax;
use crate::ui::{Bounds, Color, Context, Coordinates};

use super::{Buffer, Span};

lazy_static! {
    static ref DEFAULT_THEME: HashMap<&'static str, Color> = hashmap! {
        "attribute" => Color::new(0xff, 0x00, 0x00),
        "comment" => Color::new(0x4e, 0x4e, 0x4e),
        "constant" => Color::new(0x00, 0x87, 0x87),
        "escape" => Color::new(0xff, 0xd7, 0x00),
        "function" => Color::new(0xff, 0x87, 0x00),
        "function.macro" => Color::new(0xff, 0x00, 0x00),
        "keyword" => Color::new(0xff, 0xff, 0x00),
        "label" => Color::new(0xff, 0xff, 0x00),
        "number" => Color::new(0x00, 0x87, 0x87),
        "operator" => Color::new(0xff, 0xff, 0x00),
        "string" => Color::new(0x5f, 0x87, 0xd7),
        "type" => Color::new(0x00, 0xff, 0x00),
    };
}

pub struct Theme {
    /// Map of capture index to associated color, if any.
    colors: Vec<Option<Color>>,
}

impl Theme {
    pub fn new(capture_names: &[String]) -> Self {
        let theme = &DEFAULT_THEME;

        Self {
            colors: capture_names
                .iter()
                .map(|name| {
                    if let Some(color) = theme.get(name.as_str()) {
                        return Some(*color);
                    }

                    for (pos, _) in name.rmatch_indices('.') {
                        let fallback_name = &name[..pos];

                        if let Some(color) = theme.get(&fallback_name) {
                            info!("no color for {}, falling back to {}", name, &fallback_name);
                            return Some(*color);
                        }
                    }

                    info!("no color for {}", name);

                    None
                })
                .collect(),
        }
    }

    pub fn color_for(&self, capture_index: usize) -> Option<Color> {
        self.colors[capture_index]
    }
}

pub struct Highlighter {
    parser: RefCell<Parser>,
    query: Query,
    old_tree: Option<Tree>,
    theme: Theme,
}

impl Highlighter {
    pub fn new(language: Syntax) -> Self {
        let (language, query) = tree_sitter_highlight_config(language);

        let mut parser = Parser::new();
        parser
            .set_language(language)
            .expect("incompatible tree-sitter version");

        let theme = Theme::new(query.capture_names());

        Highlighter {
            query,
            parser: RefCell::new(parser),
            old_tree: None,
            theme,
        }
    }

    /// Apply syntax highlighting from buffer to the screen.
    pub fn highlight(&self, ctx: &mut Context, buffer: &Buffer) {
        debug!("starting highlighting");

        let tree = self.parser.borrow_mut().parse_with(
            &mut |_, point| buffer.slice_at(point),
            self.old_tree.as_ref(),
        );

        let tree = match tree {
            Some(tree) => tree,
            None => return,
        };

        let mut cursor = QueryCursor::new();

        // TODO: This should be the viewport of the buffer.
        let span = Span::from_untyped(&ctx.bounds.to_untyped().cast::<usize>());

        let (start, end) = span_to_points(span);
        cursor.set_point_range(start, end);

        let captures_query =
            cursor.captures(&self.query, tree.root_node(), |node| &buffer[node.range()]);

        for (m, _) in captures_query {
            for capture in m.captures {
                let range = capture.node.range();
                let index = capture.index as usize;

                let color = self.theme.color_for(index);

                if log_enabled!(log::Level::Debug) {
                    // The capture range may span across lines, so we can't use the buffer's
                    // `Index<Range>` implementation.
                    let text = String::from_utf8(
                        buffer
                            .bytes()
                            .skip(range.start_byte)
                            .take(range.end_byte - range.start_byte)
                            .collect(),
                    )
                    .expect("buffer must be UTF-8");

                    debug!(
                        "capture={} color={:?} text={:?}",
                        self.query.capture_names()[index],
                        color,
                        text,
                    );
                }

                if let Some(color) = color {
                    highlight_range(ctx, range, color);
                }
            }
        }

        debug!("finished highlighting");
    }
}

/// Highlights a tree-sitter range on the screen.
fn highlight_range(ctx: &mut Context, range: Range, color: Color) {
    for y in range.start_point.row..=range.end_point.row {
        let start_x = if y == range.start_point.row {
            range.start_point.column as u16
        } else {
            0
        };

        let end_x = if y == range.end_point.row {
            range.end_point.column as u16
        } else {
            ctx.bounds.max.x
        };

        let y = y as u16;
        let highlight_bounds =
            Bounds::new(Coordinates::new(start_x, y), Coordinates::new(end_x, y + 1));

        ctx.screen
            .apply_color(ctx.bounds.intersection(&highlight_bounds), color);
    }
}

impl Buffer {
    /// Return a slice of text starting at the given point.
    ///
    /// The slice returned may be of any length.
    fn slice_at<'a>(&'a self, point: Point) -> impl AsRef<[u8]> + 'a {
        // TODO: Should this take usize to support very large buffers?
        if point.row == self.lines.len() {
            return "";
        }

        let line = &self.lines[point.row];

        if point.column == line.len() {
            "\n"
        } else {
            &line[point.column..]
        }
    }
}

impl Index<Range> for Buffer {
    type Output = str;

    fn index(&self, r: Range) -> &str {
        assert!(
            r.start_point.row == r.end_point.row,
            "cannot index across rows: {:?}",
            r,
        );
        &self.lines[r.start_point.row][r.start_point.column..r.end_point.column]
    }
}

fn tree_sitter_highlight_config(language: Syntax) -> (tree_sitter::Language, Query) {
    use Syntax::*;

    match language {
        JavaScript => tree_sitter_languages::javascript(),
        Rust => tree_sitter_languages::rust(),
    }
}

fn span_to_points(span: Span) -> (Point, Point) {
    (
        Point::new(span.min.y, span.min.x),
        Point::new(span.max.y - 1, span.max.x),
    )
}

#[cfg(test)]
mod tests {
    use indoc::indoc;
    use tree_sitter::Point;

    use crate::buffer::{Buffer, Position, Span};
    use crate::ui::{Bounds, Color, Context, Drawable, Screen, Size};

    use super::{span_to_points, Syntax, Theme};

    #[test]
    fn points_from_span() {
        let span = Span::new(Position::new(0, 0), Position::new(2, 1));
        let (min, max) = span_to_points(span);

        assert_eq!(min, Point::new(0, 0));
        assert_eq!(max, Point::new(0, 2));
    }

    #[test]
    fn highlight_large_buffer() {
        let mut buffer = Buffer::from(indoc! {r#"
            fn main() {
                println!("Hello, world!")
            }
        "#});

        buffer.set_syntax(Some(Syntax::Rust));

        let mut screen = Screen::new(Size::new(5, 2));

        let mut ctx = Context {
            bounds: Bounds::from_size(screen.size),
            screen: &mut screen,
        };

        buffer.draw(&mut ctx);
    }

    #[test]
    fn highlight_at_edge_of_screen() {
        let mut buffer = Buffer::from("impl Debug for Foo {}");

        buffer.set_syntax(Some(Syntax::Rust));

        let mut screen = Screen::new(Size::new(5, 1));

        let mut ctx = Context {
            bounds: Bounds::from_size(screen.size),
            screen: &mut screen,
        };

        buffer.draw(&mut ctx);
    }

    #[test]
    fn highlight_multiline_comment() {
        let _ = env_logger::builder().is_test(true).try_init();

        let mut buffer = Buffer::from(indoc! {r#"
            /*
             * I am a multi-line comment.
             * I should be fully highlighted!
             */
        "#});

        buffer.set_syntax(Some(Syntax::JavaScript));

        let mut screen = Screen::new(Size::new(30, 5));

        let mut ctx = Context {
            bounds: Bounds::from_size(screen.size),
            screen: &mut screen,
        };

        buffer.draw(&mut ctx);

        assert!(screen[(0, 0)].color.is_some());
        assert!(screen[(0, 1)].color.is_some());
        assert!(screen[(1, 10)].color.is_some());
    }

    #[test]
    fn theme_capture_name_fallback() {
        let theme = Theme::new(&[
            String::from("function"),
            String::from("function.method"),
            String::from("function.builtin.static"),
        ]);
        // assert_eq!(theme.color_for(1), Some(Color::new(0xff, 0x87, 0x00)));
        assert_eq!(theme.color_for(2), Some(Color::new(0xff, 0x87, 0x00)));
    }
}
