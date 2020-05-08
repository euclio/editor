//! Text editing buffers and buffer management.

use std::path::PathBuf;

use euclid::Point2D;
use futures::stream::{self, StreamExt, TryStreamExt};
use itertools::Itertools;
use log::*;
use lsp_types::TextDocumentItem;
use tokio::fs::File;
use tokio::io::{self, AsyncBufReadExt, BufReader};

use crate::lsp::{LanguageId, ToUri};
use crate::ui::{Context, Coordinates, Drawable};

/// Unit for buffer-internal positions and lengths.
pub struct BufferSpace;

/// Cursor position within a buffer.
pub type Cursor = Point2D<u16, BufferSpace>;

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
    pub cursor: Cursor,
    pub language_id: Option<LanguageId>,
}

impl Buffer {
    pub fn new() -> Self {
        Buffer {
            path: None,
            cursor: Cursor::default(),
            lines: vec![String::new()],
            language_id: None,
        }
    }

    pub async fn open(path: PathBuf) -> io::Result<Self> {
        info!("creating buffer with contents of {}", path.display());

        let reader = BufReader::new(File::open(&path).await?);

        Ok(Buffer {
            cursor: Cursor::default(),
            lines: reader.lines().try_collect().await?,
            path: Some(path),
            language_id: Some(LanguageId::Rust),
        })
    }

    pub fn to_text_document_item(&self) -> Option<TextDocumentItem> {
        Some(TextDocumentItem {
            uri: self.path.as_ref()?.to_uri(),
            language_id: self
                .language_id
                .expect("language must be known to convert to text document item")
                .to_string(),
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
            cursor: Cursor::default(),
            language_id: None,
            lines: s.lines().map(|line| line.to_owned()).collect(),
            path: None,
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
