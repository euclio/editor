use std::os::unix::io::FromRawFd;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::thread;

use anyhow::Error;
use bytes::{Buf, BytesMut};
use futures::Stream;
use lazy_static::lazy_static;
use libc::STDIN_FILENO;
use log::*;
use nix::sys::termios::{self, ControlFlags, InputFlags, LocalFlags, OutputFlags, SetArg, Termios};
use pin_project::{pin_project, pinned_drop};
use qp_trie::Trie;
use tokio::fs::File;
use tokio::io;
use tokio_util::codec::{Decoder, FramedRead};

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Key {
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    Char(char),
    Ctrl(char),
    Backspace,
    Return,
    Esc,
}

lazy_static! {
    /// Trie mapping all known escape sequences to a pair of the Key that the represent and the
    /// length of the sequence.
    static ref ESCAPE_SEQUENCES: Trie<&'static [u8], Key> = {
        use Key::*;

        macro_rules! init_trie {
            ( $( $seq:literal => $key:expr ),* $(,)? ) => {
                {

                    let mut trie = Trie::new();

                    $(
                        trie.insert(&$seq[..], $key);
                    )*

                    trie
                }
            }
        }

        init_trie! {
            b"A" => ArrowUp,
            b"B" => ArrowDown,
            b"C" => ArrowRight,
            b"D" => ArrowLeft,
        }
    };
}

/// Codec to decode keys from buffers containing ANSI escape sequences from stdin. Doing this is
/// notoriously ambiguous. The strategy employed by this codec relies on a few facts:
///
/// - All escape sequences start with `\x1b[`.
/// - User input is slow compared to the speed of processing, so individual inputs will
///   generally arrive in their own buffers.
/// - There are a finite number of known escape sequences, so try to parse from a subset if
///   there's ambiguity.
struct KeyCodec;

impl KeyCodec {
    fn parse_byte(byte: u8) -> Key {
        #[allow(clippy::match_overlapping_arm)] // rust-lang/rust-clippy#6603
        match byte {
            b'\x0D' => Key::Return,
            b'\x01'..=b'\x1A' => Key::Ctrl((byte | 0x60) as char),
            b'\x1b' => Key::Esc,
            b'\x7f' => Key::Backspace,
            _ => Key::Char(byte as char),
        }
    }

    /// Attempts to parse a key from a byte slice that starts with an escape sequence.
    ///
    /// The sequence should have its `\x1b[` prefix already removed, but trailing bytes are
    /// allowed. If the slice contains a known escape sequence, then this function returns a pair
    /// of the parsed key and how many bytes should be consumed. If no known sequence was found,
    /// `None` is returned.
    fn parse_escape_sequence(seq: &[u8]) -> Option<(Key, usize)> {
        let common_prefix = ESCAPE_SEQUENCES.longest_common_prefix(seq);
        let key = ESCAPE_SEQUENCES.get(common_prefix)?;
        Some((*key, common_prefix.len()))
    }
}

impl Decoder for KeyCodec {
    type Item = Key;
    type Error = io::Error;

    fn decode(&mut self, buf: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let key = match buf.as_ref() {
            [] => return Ok(None),
            [b'\x1b', b'[', seq @ ..] => {
                let pos = seq
                    .iter()
                    .position(|&b| b == b'\x1b')
                    .unwrap_or_else(|| seq.len());
                if let Some((key, len)) = Self::parse_escape_sequence(&seq[..pos]) {
                    buf.advance(2 + len);
                    key
                } else {
                    warn!(
                        "encountered unknown escape sequence: \\x1b[{}",
                        String::from_utf8_lossy(seq)
                    );
                    buf.advance(2 + pos);
                    return Ok(None);
                }
            }
            _ => {
                let byte = buf.split_to(1)[0];
                Self::parse_byte(byte)
            }
        };

        Ok(Some(key))
    }
}

#[pin_project(PinnedDrop)]
pub struct Stdin {
    #[pin]
    stdin: FramedRead<File, KeyCodec>,

    /// The terminal settings when the program started.
    pub old_termios: Termios,
}

impl Stdin {
    /// Creates a new Stdin instance. This function also handles entering raw mode, and the
    /// destructor will restore the original terminal settings.
    pub fn new() -> Result<Self, Error> {
        let stdin = File::from_std(unsafe { std::fs::File::from_raw_fd(STDIN_FILENO) });
        let old_termios = termios::tcgetattr(STDIN_FILENO)?;

        let mut raw = old_termios.clone();
        raw.input_flags.remove(
            InputFlags::BRKINT
                | InputFlags::ICRNL
                | InputFlags::INPCK
                | InputFlags::ISTRIP
                | InputFlags::IXON,
        );
        raw.output_flags.remove(OutputFlags::OPOST);
        raw.control_flags.insert(ControlFlags::CS8);
        raw.local_flags
            .remove(LocalFlags::ECHO | LocalFlags::ICANON | LocalFlags::IEXTEN | LocalFlags::ISIG);
        termios::tcsetattr(STDIN_FILENO, SetArg::TCSAFLUSH, &raw)?;

        Ok(Stdin {
            stdin: FramedRead::new(stdin, KeyCodec),
            old_termios,
        })
    }
}

impl Stream for Stdin {
    type Item = io::Result<Key>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
        self.project().stdin.poll_next(cx)
    }
}

#[pinned_drop]
impl PinnedDrop for Stdin {
    fn drop(self: Pin<&mut Self>) {
        if !thread::panicking() {
            let _ = termios::tcsetattr(STDIN_FILENO, SetArg::TCSAFLUSH, &self.old_termios);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use futures::TryStreamExt;
    use tokio_util::codec::FramedRead;

    use super::{Key, KeyCodec};

    #[tokio::test]
    async fn decode_char() {
        let keys: Vec<Key> = FramedRead::new(Cursor::new(b"a"), KeyCodec)
            .try_collect()
            .await
            .unwrap();

        assert_eq!(keys, vec![Key::Char('a')]);
    }

    #[tokio::test]
    async fn decode_ctrl() {
        let keys: Vec<Key> = FramedRead::new(Cursor::new(b"\x01"), KeyCodec)
            .try_collect()
            .await
            .unwrap();

        assert_eq!(keys, vec![Key::Ctrl('a')]);
    }

    #[tokio::test]
    async fn decode_escape() {
        let keys: Vec<Key> = FramedRead::new(Cursor::new(b"\x1b"), KeyCodec)
            .try_collect()
            .await
            .unwrap();

        assert_eq!(keys, vec![Key::Esc]);
    }

    #[tokio::test]
    async fn decode_escape_seq() {
        let keys: Vec<Key> = FramedRead::new(Cursor::new(b"\x1b[A"), KeyCodec)
            .try_collect()
            .await
            .unwrap();

        assert_eq!(keys, vec![Key::ArrowUp]);
    }

    #[tokio::test]
    async fn decode_multi_char() {
        let keys: Vec<Key> = FramedRead::new(Cursor::new(b"TeSt"), KeyCodec)
            .try_collect()
            .await
            .unwrap();

        assert_eq!(
            keys,
            vec![
                Key::Char('T'),
                Key::Char('e'),
                Key::Char('S'),
                Key::Char('t')
            ]
        );
    }

    #[tokio::test]
    async fn decode_multi_escape_seq() {
        let keys: Vec<Key> = FramedRead::new(Cursor::new(b"\x1b[B\x1b[A"), KeyCodec)
            .try_collect()
            .await
            .unwrap();

        assert_eq!(keys, vec![Key::ArrowDown, Key::ArrowUp]);
    }

    #[tokio::test]
    async fn decode_escape_then_char() {
        // This case is actually pretty hard to reproduce, but it is possible.
        let keys: Vec<Key> = FramedRead::new(Cursor::new(b"\x1b[Bf"), KeyCodec)
            .try_collect()
            .await
            .unwrap();

        assert_eq!(keys, vec![Key::ArrowDown, Key::Char('f')])
    }

    #[tokio::test]
    async fn unknown_escape_sequence() {
        let keys: Vec<Key> = FramedRead::new(Cursor::new(b"\x1b[1337"), KeyCodec)
            .try_collect()
            .await
            .unwrap();

        assert_eq!(keys, vec![]);
    }
}
