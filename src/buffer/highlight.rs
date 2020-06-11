//! Syntax highlighting for buffer contents.

use std::cell::RefCell;
use std::ops::Index;

use log::*;
use tree_sitter::{Parser, Point, Query, QueryCursor, Tree, Range};

use crate::syntax::Syntax;
use crate::ui::Color;

use super::{Buffer, Position, Span};

pub struct Theme {
    _priv: (),
}

impl Theme {
    pub fn new() -> Self {
        Theme {
            _priv: ()
        }
    }

    pub fn color_for(&self, name: &str) -> Option<Color> {
        if name.starts_with("keyword") {
            Some(Color::new(0xff, 0xff, 0x00))
        } else if name.starts_with("function") {
            Some(Color::new(0xff, 0x87, 0x00))
        } else if name.starts_with("type") {
            Some(Color::new(0x00, 0xff, 0x00))
        } else if name.starts_with("number") {
            Some(Color::new(0x00, 0x87, 0x87))
        } else if name.starts_with("string") {
            Some(Color::new(0x5f, 0x87, 0xd7))
        } else {
            None
        }
    }
}

pub struct Highlighter {
    parser: RefCell<Parser>,
    query: Query,
    old_tree: Option<Tree>,
    theme: Theme,
}

impl Highlighter {
    pub fn new(language: Option<Syntax>) -> Self {
        let (language, query) = tree_sitter_highlight_config(language.unwrap());

        let mut parser = Parser::new();
        parser.set_language(language).expect("incompatible tree-sitter version");

        Highlighter {
            parser: RefCell::new(parser),
            old_tree: None,
            query,
            theme: Theme::new(),
        }
    }

    pub fn highlights(&self, buffer: &Buffer, span: Span, mut f: impl FnMut(Span, Color)) {
        debug!("starting highlighting");

        let tree = self.parser.borrow_mut().parse_with(&mut |_, point| {
            buffer.slice_at(point)
        }, self.old_tree.as_ref());

        let tree = match tree {
            Some(tree) => tree,
            None => return,
        };

        let mut cursor = QueryCursor::new();
        cursor.set_point_range(
            Point {
                row: span.min.y,
                column: span.min.x,
            },
            Point {
                row: span.max.y,
                column: span.max.x,
            },
        );
        let captures_query = cursor.captures(&self.query, tree.root_node(), |node| {
            &buffer[node.range()]
        });

        let capture_names = self.query.capture_names();

        for (m, _) in captures_query {
            for capture in m.captures {
                let node = capture.node;
                let range = node.range();
                let idx = capture.index;
                let name = &capture_names[idx as usize];

                let color = self.theme.color_for(name);
                debug!("capture={} color={:?} text={}", name, color, &buffer[range]);

                if let Some(color) = self.theme.color_for(name) {
                    f(range.to_span(), color);
                }
            }
        }

        debug!("finished highlighting");
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
        assert!(r.start_point.row == r.end_point.row, "cannot index across rows");
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

trait RangeExt {
    fn to_span(self) -> Span;
}

impl RangeExt for Range {
    fn to_span(self) -> Span {
        let start = Position::new(self.start_point.column, self.start_point.row);
        let end = Position::new(self.end_point.column, self.end_point.row);

        Span::new(start, end)
    }
}

#[cfg(test)]
mod tests {
    use tree_sitter::{Point, Range};

    use super::RangeExt;

    #[test]
    fn span_conversion() {
        let range = Range {
            start_byte: 0,
            end_byte: 3,
            start_point: Point { row: 0, column: 0 },
            end_point: Point { row: 0, column: 3 ,}
        };

        let span = range.to_span();
        assert_eq!(span.width(), 3);
    }
}
