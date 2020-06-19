//! Terminal I/O.

use std::mem::MaybeUninit;
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use std::thread;

use anyhow::{Context, Error};
use libc::STDOUT_FILENO;
use log::*;
use nix::ioctl_read_bad;
use terminfo::{capability as cap, expand};
use tokio::fs::File;
use tokio::io::{self, AsyncWriteExt, BufWriter};

use crate::ui::{Coordinates, Screen, Size};

mod input;

pub use input::{Key, Stdin};

pub struct Terminal {
    terminfo: terminfo::Database,
    stdout: BufWriter<File>,
    /// The screen that should be drawn on the next refresh.
    back: Screen,
    pub cursor: Coordinates,
}

impl Terminal {
    pub async fn new() -> Result<Self, Error> {
        let mut stdout = File::from_std(unsafe { std::fs::File::from_raw_fd(STDOUT_FILENO) });

        let terminfo = terminfo::Database::from_env().context("failed to initialize terminfo")?;

        if let Some(smcup) = terminfo.get::<cap::EnterCaMode>() {
            stdout.write_all(smcup.as_ref()).await?;
        }

        let size = get_size(stdout.as_raw_fd())?;

        Ok(Terminal {
            terminfo,
            stdout: BufWriter::new(stdout),
            back: Screen::new(size),
            cursor: Coordinates::zero(),
        })
    }

    /// Returns a sequence of bytes that can be used to restore the terminal to its original state.
    /// This does *not* include the TTY settings, `input::Stdin` is responsible for that.
    pub fn restore_sequence(&self) -> Vec<u8> {
        let mut seq = vec![];

        if let Some(rmcup) = self.terminfo.get::<cap::ExitCaMode>() {
            seq.extend_from_slice(rmcup.as_ref());
        } else {
            warn!("no rmcup capability in terminfo");
        }

        if let Some(cnorm) = self.terminfo.get::<cap::CursorNormal>() {
            seq.extend_from_slice(cnorm.as_ref());
        } else {
            warn!("no cnorm capability in terminfo");
        }

        seq
    }

    pub fn screen(&mut self) -> &mut Screen {
        &mut self.back
    }

    pub fn size(&self) -> Size {
        self.back.size
    }

    pub fn refresh_size(&mut self) -> Result<Size, Error> {
        self.back.size = get_size(self.stdout.get_ref().as_raw_fd())?;
        Ok(self.size())
    }

    pub async fn refresh(&mut self) -> io::Result<()> {
        self.hide_cursor().await?;

        if let Some(cl) = self.terminfo.get::<cap::ClearScreen>() {
            self.stdout.write_all(cl.as_ref()).await?;
        }

        let mut last_color = None;

        {
            let mut rows = self.back.iter_rows().peekable();
            while let Some(row) = rows.next() {
                for col in row {
                    if col.color != last_color {
                        match col.color {
                            Some(color) => {
                                self.stdout
                                    .write_all(
                                        format!("\x1b[38;2;{};{};{}m", color.r, color.g, color.b)
                                            .as_bytes(),
                                    )
                                    .await?;
                            }
                            None => {
                                let sgr0 = self.terminfo.get::<cap::ExitAttributeMode>().unwrap();
                                self.stdout.write_all(sgr0.as_ref()).await?;
                            }
                        }

                        last_color = col.color;
                    }

                    // FIXME: Doesn't support non-ASCII
                    self.stdout.write_u8(col.c as u8).await?;
                }

                if rows.peek().is_some() {
                    self.stdout.write_all(b"\r\n").await?;
                }
            }
        }

        let cup = expand!(self
            .terminfo
            .get::<cap::CursorAddress>().unwrap().as_ref();
            self.cursor.y, self.cursor.x)
        .unwrap();
        self.stdout.write_all(&cup).await?;

        self.show_cursor().await?;

        self.stdout.flush().await
    }

    async fn hide_cursor(&mut self) -> io::Result<()> {
        let civis = expand!(self
            .terminfo
            .get::<cap::CursorInvisible>()
            .unwrap()
            .as_ref())
        .unwrap();
        self.stdout.write_all(&civis).await
    }

    async fn show_cursor(&mut self) -> io::Result<()> {
        let cnorm = expand!(self.terminfo.get::<cap::CursorNormal>().unwrap().as_ref()).unwrap();
        self.stdout.write_all(&cnorm).await
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        if !thread::panicking() {
            let _ = futures::executor::block_on(async move {
                let seq = self.restore_sequence();
                self.stdout.write_all(&seq).await?;
                self.stdout.flush().await?;

                Ok::<(), io::Error>(())
            });
        }
    }
}

/// Queries the terminal size on a file descriptor.
fn get_size(fd: RawFd) -> nix::Result<Size> {
    ioctl_read_bad!(tiocgwinsz, libc::TIOCGWINSZ, libc::winsize);

    let size = unsafe {
        let mut winsize = MaybeUninit::zeroed();
        tiocgwinsz(fd, winsize.as_mut_ptr())?;
        winsize.assume_init()
    };
    Ok(Size::new(size.ws_col, size.ws_row))
}
