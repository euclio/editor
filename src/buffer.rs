//! Text editing buffers and buffer management.

use std::cmp;
use std::iter;
use std::path::PathBuf;

use euclid::{Point2D, Rect};
use futures::stream::{self, StreamExt, TryStreamExt};
use itertools::Itertools;
use log::*;
use lsp_types::TextDocumentItem;
use tokio::fs::File;
use tokio::io::{self, AsyncBufReadExt, BufReader};

use crate::lsp::ToUri;
use crate::syntax::Syntax;
use crate::ui::{Bounds, Color, Context, Coordinates, Drawable};

mod highlight;
mod motion;

use highlight::Highlighter;
use motion::Cursor;

/// Unit for buffer-internal positions and lengths.
pub struct BufferSpace;

/// A position within a buffer.
pub type Position = Point2D<usize, BufferSpace>;

/// A translation within a buffer. Used for cursor and viewport movement.
pub type Offset = euclid::Vector2D<isize, BufferSpace>;

/// A rectangular area of text.
///
/// This area is endpoint-exclusive.
pub type Span = Rect<usize, BufferSpace>;

/// Container for all open buffers.
///
/// Also keeps track of which buffer is considered the current (or active) buffer.
pub struct Buffers {
    buffers: Vec<Buffer>,
    current: usize,
}

impl Buffers {
    pub async fn from_paths(paths: Vec<PathBuf>, bounds: Bounds) -> io::Result<Self> {
        let mut buffers = if paths.is_empty() {
            Buffers {
                buffers: vec![Buffer::new()],
                current: 0,
            }
        } else {
            let buffers = stream::iter(paths).then(Buffer::open).try_collect().await?;

            Buffers {
                buffers,
                current: 0,
            }
        };

        buffers.current_mut().viewport = Some(bounds.to_rect().to_usize().cast_unit());

        info!(
            "active buffer viewport: {:?}",
            buffers.current_mut().viewport
        );

        Ok(buffers)
    }

    /// The active buffer.
    pub fn current(&self) -> &Buffer {
        &self.buffers[self.current]
    }

    /// The active buffer, borrowed mutably.
    pub fn current_mut(&mut self) -> &mut Buffer {
        &mut self.buffers[self.current]
    }
}

impl<'a> IntoIterator for &'a Buffers {
    type Item = &'a Buffer;
    type IntoIter = std::slice::Iter<'a, Buffer>;

    fn into_iter(self) -> Self::IntoIter {
        self.buffers.iter()
    }
}

/// An in-memory view of a file.
pub struct Buffer {
    /// The file path that this buffer represents.
    path: Option<PathBuf>,

    /// The lines of the file.
    lines: Vec<String>,

    /// The cursor position within the buffer.
    ///
    /// The on-screen cursor location is determined by offsetting this position with the viewport.
    cursor: Cursor,

    /// Syntax associated with the buffer.
    ///
    /// `None` if unknown or plain-text.
    pub syntax: Option<Syntax>,

    /// Responsible for highlighting, if a supported syntax was detected.
    highlighter: Option<Highlighter>,

    /// The visible portion of the buffer.
    ///
    /// `None` if the buffer is hidden.
    viewport: Option<Span>,
}

impl Buffer {
    pub fn new() -> Self {
        Buffer {
            path: None,
            cursor: Cursor::default(),
            lines: vec![String::new()],
            syntax: None,
            highlighter: None,
            viewport: None,
        }
    }

    pub fn set_syntax(&mut self, syntax: Option<Syntax>) {
        self.syntax = syntax;
        self.highlighter = syntax.map(Highlighter::new);
    }

    pub async fn open(path: PathBuf) -> io::Result<Self> {
        info!("creating buffer with contents of {}", path.display());

        let reader = BufReader::new(File::open(&path).await?);

        let syntax = Syntax::identify(&path);
        info!("syntax identified: {:?}", syntax);

        Ok(Buffer {
            cursor: Cursor::default(),
            lines: reader.lines().try_collect().await?,
            path: Some(path),
            syntax,
            highlighter: syntax.map(Highlighter::new),
            viewport: None,
        })
    }

    pub fn to_text_document_item(&self) -> Option<TextDocumentItem> {
        Some(TextDocumentItem {
            uri: self.path.as_ref()?.to_uri(),
            language_id: self
                .syntax
                .expect("language must be known to convert to text document item")
                .into_language_id()
                .to_owned(),
            version: 0,
            text: self.lines.join("\n"),
        })
    }

    pub fn bytes(&self) -> impl Iterator<Item = u8> + '_ {
        self.lines
            .iter()
            .flat_map(|line| line.bytes().chain(iter::once(b'\n')))
    }

    /// Returns the cursor position relative to the viewport.
    pub fn cursor_position(&self) -> Position {
        let viewport = self
            .viewport
            .expect("attempted to determine cursor position for hidden buffer");

        Position::new(self.cursor.x(), self.cursor.y() - viewport.min_y())
    }
}

impl Default for Buffer {
    fn default() -> Self {
        Buffer::new()
    }
}

impl<'a> From<&'a str> for Buffer {
    fn from(s: &str) -> Self {
        Buffer {
            cursor: Cursor::default(),
            syntax: None,
            lines: s.lines().map(|line| line.to_owned()).collect(),
            path: None,
            highlighter: None,
            viewport: None,
        }
    }
}

impl Drawable for Buffer {
    fn draw(&self, ctx: &mut Context<'_>) {
        let viewport = match self.viewport {
            Some(viewport) => viewport,
            None => return,
        };

        let tilde = String::from("~");

        for (row, line) in self
            .lines
            .iter()
            .skip(viewport.min_y())
            .pad_using(viewport.height(), |_| &tilde)
            .enumerate()
            .take(viewport.height())
        {
            // FIXME: Naively assumes ASCII.
            let max = cmp::min(viewport.max_x(), line.len());
            let line = &line[viewport.min_x()..max];
            ctx.screen.write(Coordinates::new(0, row as u16), line);
        }

        for row in (self.lines.len() - viewport.min_y())..ctx.bounds.height().into() {
            let bounds = Bounds::new(
                Coordinates::new(0, row as u16),
                Coordinates::new(1, row as u16 + 1),
            );

            ctx.screen.apply_color(bounds, Color::BLUE);
        }

        if let Some(highlighter) = &self.highlighter {
            highlighter.highlight(&mut ctx.screen, &self);
        }
    }
}

#[cfg(test)]
mod tests {
    use euclid::rect;
    use indoc::indoc;

    use crate::ui::{Bounds, Context, Drawable, Screen, Size};

    use super::{Buffer, Span};

    #[test]
    fn bytes_iter() {
        let buffer = Buffer::from(indoc! {"
            Lorem
            Ipsum
        "});

        assert_eq!(buffer.bytes().collect::<Vec<_>>(), b"Lorem\nIpsum\n",);
    }

    #[test]
    fn draw_empty_buffer() {
        let mut buffer = Buffer::new();

        let size = Size::new(2, 3);
        let mut screen = Screen::new(size);

        let mut ctx = Context {
            bounds: Bounds::from_size(size),
            screen: &mut screen,
        };

        buffer.viewport = Some(Span::from_size(size.cast().cast_unit()));

        buffer.draw(&mut ctx);

        assert_eq!(screen[(0, 0)].c, ' ');
        assert_eq!(screen[(1, 0)].c, '~');
        assert_eq!(screen[(1, 1)].c, ' ');
        assert_eq!(screen[(2, 0)].c, '~');
    }

    #[test]
    fn draw_long_buffer() {
        let mut buffer = Buffer::from(indoc!(
            r"foo
            bar
            baz"
        ));

        let size = Size::new(5, 2);
        let mut screen = Screen::new(size);

        let mut ctx = Context {
            bounds: Bounds::from_size(size),
            screen: &mut screen,
        };

        buffer.viewport = Some(Span::from_size(size.cast().cast_unit()));
        buffer.draw(&mut ctx);

        assert_eq!(screen[(0, 0)].c, 'f');
        assert_eq!(screen[(1, 0)].c, 'b');
    }

    #[test]
    fn draw_buffer_offset_viewport() {
        let mut buffer = Buffer::from(indoc! {"
            abcde
            fghij
            klmno
            pqrst
            uvwxy
        "});

        buffer.viewport = Some(rect(1, 1, 3, 3));

        let mut screen = Screen::new(Size::new(3, 3));

        let mut ctx = Context {
            bounds: Bounds::from_size(screen.size),
            screen: &mut screen,
        };

        buffer.draw(&mut ctx);

        assert_eq!(screen[(0, 0)].c, 'g');
        assert_eq!(screen[(2, 2)].c, 's');
    }
}
