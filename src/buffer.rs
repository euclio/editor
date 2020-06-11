//! Text editing buffers and buffer management.

use std::path::PathBuf;

use euclid::{Box2D, Point2D};
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

use highlight::Highlighter;

/// Unit for buffer-internal positions and lengths.
pub struct BufferSpace;

/// A position within a buffer.
pub type Position = Point2D<usize, BufferSpace>;

/// A rectangular area of text.
pub type Span = Box2D<usize, BufferSpace>;

/// Container for all open buffers.
///
/// Also keeps track of which buffer is considered the current (or active) buffer.
pub struct Buffers {
    buffers: Vec<Buffer>,
    current: usize,
}

impl Buffers {
    pub async fn from_paths(paths: Vec<PathBuf>) -> io::Result<Self> {
        let buffers = if paths.is_empty() {
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

        Ok(buffers)
    }

    pub fn current(&self) -> &Buffer {
        &self.buffers[self.current]
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

    /// The cursor position within the buffer. May or may not correspond to the on-screen cursor.
    pub cursor: Position,

    /// Syntax associated with the buffer.
    ///
    /// `None` if unknown or plain-text.
    pub syntax: Option<Syntax>,

    highlighter: Highlighter,
}

impl Buffer {
    pub fn new() -> Self {
        Buffer {
            path: None,
            cursor: Position::default(),
            lines: vec![String::new()],
            syntax: None,
            highlighter: Highlighter::new(None),
        }
    }

    pub async fn open(path: PathBuf) -> io::Result<Self> {
        info!("creating buffer with contents of {}", path.display());

        let reader = BufReader::new(File::open(&path).await?);

        let syntax = Syntax::identify(&path);
        info!("syntax identified: {:?}", syntax);

        Ok(Buffer {
            cursor: Position::default(),
            lines: reader.lines().try_collect().await?,
            path: Some(path),
            syntax,
            highlighter: Highlighter::new(syntax),
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
}

impl Default for Buffer {
    fn default() -> Self {
        Buffer::new()
    }
}

impl<'a> From<&'a str> for Buffer {
    fn from(s: &str) -> Self {
        Buffer {
            cursor: Position::default(),
            syntax: None,
            lines: s.lines().map(|line| line.to_owned()).collect(),
            path: None,
            highlighter: Highlighter::new(None),
        }
    }
}

impl Drawable for Buffer {
    fn draw(&self, ctx: &mut Context<'_>) {
        let tilde = String::from("~");

        for (row, line) in self
            .lines
            .iter()
            .pad_using(ctx.bounds.height().into(), |_| &tilde)
            .enumerate()
            .take(ctx.bounds.height().into())
        {
            ctx.screen.write(Coordinates::new(0, row as u16), line);
        }

        let tilde = String::from("~");

        for (row, line) in self
            .lines
            .iter()
            .pad_using(ctx.bounds.height().into(), |_| &tilde)
            .enumerate()
            .take(ctx.bounds.height().into())
        {
            ctx.screen.write(Coordinates::new(0, row as u16), line);
        }

        for row in self.lines.len()..ctx.bounds.height().into() {
            let bounds = Bounds::new(
                Coordinates::new(0, row as u16),
                Coordinates::new(1, row as u16),
            );

            ctx.screen.apply_color(bounds, Color::BLUE);
        }

        self.highlighter.highlights(
            &self,
            Span::from_untyped(&ctx.bounds.to_untyped().cast::<usize>()),
            |span, color| {
                let bounds = Bounds::new(
                    Coordinates::new(span.min.x as u16, span.min.y as u16),
                    Coordinates::new(span.max.x as u16, span.max.y as u16),
                );

                ctx.screen.apply_color(bounds, color)
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use crate::ui::{Bounds, Context, Drawable, Screen, Size};

    use super::Buffer;

    #[test]
    fn draw_long_buffer() {
        let buffer = Buffer::from(
            r"foo
            bar
            baz",
        );

        let mut screen = Screen::new(Size::new(5, 2));

        let mut ctx = Context {
            bounds: Bounds::from_size(screen.size),
            screen: &mut screen,
        };

        buffer.draw(&mut ctx);
    }
}
