//! Text editing buffers and buffer management.

use std::cmp;
use std::env;
use std::path::PathBuf;

use euclid::{Point2D, Rect};
use futures::stream::{self, StreamExt, TryStreamExt};
use itertools::Itertools;
use log::*;
use lsp_types::{TextDocumentItem, VersionedTextDocumentIdentifier};
use tokio::fs::{self, File};
use tokio::io::{self, AsyncBufReadExt, BufReader};

use crate::lsp::ToUri;
use crate::syntax::Syntax;
use crate::ui::{Bounds, Color, Context, Coordinates, Drawable};

mod edit;
mod highlight;
mod motion;
mod storage;
mod units;

use highlight::Highlighter;
use motion::Cursor;
use storage::Storage;

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
            let buffers = stream::iter(paths)
                .then(|mut path| async {
                    if !path.is_absolute() {
                        match env::current_dir() {
                            Ok(dir) => path = dir.join(path),
                            Err(e) => return Err(e),
                        }
                    }

                    Buffer::open(path).await
                })
                .try_collect()
                .await?;

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

    /// Buffer contents.
    storage: Storage,

    /// The version of the document. Increases after each edit, including undo/redo.
    version: u32,

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
            storage: Storage::new(),
            version: 0,
            syntax: None,
            highlighter: None,
            viewport: None,
        }
    }

    pub fn set_syntax(&mut self, syntax: Option<Syntax>) {
        self.syntax = syntax;
        self.highlighter = syntax.map(Highlighter::new);
    }

    /// Open a new buffer containing the contents of the given path. The path must be absolute.
    pub async fn open(path: PathBuf) -> io::Result<Self> {
        info!("creating buffer for {}", path.display());

        assert!(path.is_absolute(), "path must be absolute");

        let lines = if fs::metadata(&path).await.is_ok() {
            let reader = BufReader::new(File::open(&path).await?);
            reader.lines().try_collect().await?
        } else {
            info!("{} does not exist", path.display());
            vec![String::new()]
        };

        info!("read {} lines", lines.len());

        let syntax = Syntax::identify(&path);
        info!("syntax identified: {:?}", syntax);

        Ok(Buffer {
            cursor: Cursor::default(),
            storage: lines.into(),
            version: 0,
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
            version: self.version.into(),
            text: self.storage.to_string(),
        })
    }

    pub fn to_versioned_text_document_identifier(&self) -> Option<VersionedTextDocumentIdentifier> {
        Some(VersionedTextDocumentIdentifier {
            uri: self.path.as_ref()?.to_uri(),
            version: Some(self.version.into()),
        })
    }

    /// Returns the cursor position relative to the viewport.
    pub fn cursor_position(&self) -> Position {
        let viewport = self
            .viewport
            .expect("attempted to determine cursor position for hidden buffer");

        Position::new(
            self.cursor.x() - viewport.min_x(),
            self.cursor.y() - viewport.min_y(),
        )
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
            storage: Storage::from(s),
            version: 0,
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
            .storage
            .iter_lines()
            .skip(viewport.min_y())
            .pad_using(viewport.height(), |_| &tilde)
            .enumerate()
            .take(viewport.height())
        {
            // FIXME: Naively assumes ASCII.
            if viewport.min_x() < line.len() {
                let max = cmp::min(viewport.max_x(), line.len());
                let line = &line[viewport.min_x()..max];
                ctx.screen.write(Coordinates::new(0, row as u16), line);
            }
        }

        for row in (self.storage.lines() - viewport.min_y())..ctx.bounds.height().into() {
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
    use std::path::PathBuf;

    use euclid::rect;
    use indoc::indoc;

    use crate::ui::{Bounds, Context, Drawable, Screen, Size};

    use super::{Buffer, Buffers, Cursor, Position, Span, Storage};

    #[tokio::test]
    async fn buffers_open_existing_path() {
        let buffers = Buffers::from_paths(vec![PathBuf::from("src/lib.rs")], Bounds::zero())
            .await
            .unwrap();

        assert!(buffers.current().path.as_ref().unwrap().is_absolute());
        assert!(buffers.current().to_text_document_item().is_some());
    }

    #[tokio::test]
    async fn buffers_open_new_path() {
        let buffers = Buffers::from_paths(vec![PathBuf::from("does_not_exist.rs")], Bounds::zero())
            .await
            .unwrap();

        let current = buffers.current();

        assert!(current.path.as_ref().unwrap().is_absolute());
        assert!(current.to_text_document_item().is_some());
        assert_eq!(current.storage, Storage::new());
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

    #[test]
    fn draw_buffer_offset_viewport_short_line() {
        let mut buffer = Buffer::from(indoc! {"
            hello

        "});

        buffer.viewport = Some(rect(1, 0, 2, 2));

        let mut screen = Screen::new(Size::new(2, 2));

        let mut ctx = Context {
            bounds: Bounds::from_size(screen.size),
            screen: &mut screen,
        };

        buffer.draw(&mut ctx);

        println!("{:?}", screen);

        assert_eq!(screen[(0, 0)].c, 'e');
        assert_eq!(screen[(0, 1)].c, 'l');
        assert_eq!(screen[(1, 0)].c, ' ');
    }

    #[test]
    fn cursor_position() {
        let mut buffer = Buffer::from(indoc! {"
            foo
            bar
            baz
        "});

        buffer.cursor = Cursor::at(1, 1);
        buffer.viewport = Some(rect(1, 1, 1, 1));

        assert_eq!(buffer.cursor_position(), Position::zero());
    }
}
