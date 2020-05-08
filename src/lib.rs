#![recursion_limit = "256"]
#![warn(clippy::todo)]
#![warn(clippy::unwrap_used)]
#![allow(clippy::unknown_clippy_lints)] // unwrap_used isn't available on stable yet.

// workaround for rust-lang/rust#55779
extern crate serde;

use std::env;
use std::os::unix::io::AsRawFd;
use std::panic;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::Error;
use futures::channel::mpsc;
use futures::{select, StreamExt};
use log::*;
use nix::sys::termios::{self, SetArg};
use structopt::StructOpt;

mod buffer;
mod config;
mod logger;
mod lsp;
mod term;
mod ui;

use buffer::Buffers;
use config::Config;
use lsp::{LanguageServerBridge, Message, Response, ToUri};
use term::{Key, Stdin, Terminal};
use tokio::signal::unix::{signal, SignalKind};
use ui::{Bounds, Coordinates, Drawable};

pub use logger::Logger;

/// Command-line options.
#[derive(Debug, StructOpt)]
pub struct Options {
    /// A list of filenames to edit.
    files: Vec<PathBuf>,
}

pub async fn run(mut options: Options) -> Result<(), Error> {
    let Config {
        language_server_config,
    } = match Config::read(Config::config_path()).await {
        Ok(config) => config,
        Err(e) => {
            // TODO: Report error to user
            info!("unable to read config file: {}", e);
            Config::default()
        }
    };

    for path in &mut options.files {
        *path = path.canonicalize()?;
    }

    let (ls_tx, ls_rx) = mpsc::channel(10);

    let buffers = Buffers::from_paths(options.files.clone()).await?;

    let editor = Editor {
        current_dir: env::current_dir()?,
        buffers,
        ls_bridge: LanguageServerBridge::new(language_server_config, ls_tx),
        language_server_messages: ls_rx,
    };

    editor.run().await
}

/// Core editor state.
pub struct Editor {
    current_dir: PathBuf,
    buffers: Buffers,
    ls_bridge: LanguageServerBridge,

    /// Receiver for requests and notifications from language servers.
    language_server_messages: mpsc::Receiver<(lsp::Context, lsp::Message)>,
}

impl Editor {
    async fn run(mut self) -> Result<(), Error> {
        let stdin = Stdin::new()?;
        let mut term = Terminal::new().await?;

        set_panic_hook(&stdin, &term);

        let mut stdin = stdin.fuse();
        let mut sigwinch = signal(SignalKind::window_change())?.fuse();

        if let Some(ctx) = self.ls_context() {
            if let Some(server) = self.ls_bridge.server(ctx).await {
                for buffer in &self.buffers {
                    if let Some(text_document) = buffer.to_text_document_item() {
                        server.did_open_text_document(text_document).await?;
                    }
                }
            }
        }

        loop {
            // TODO: Move to default?
            self.redraw(&mut term).await?;

            select! {
                _ = sigwinch.next() => {
                    let size = term.refresh_size()?;
                    info!("received SIGWINCH, new size: {}", size);
                    self.redraw(&mut term).await?;
                }

                input = stdin.next() => {
                    let key = match input {
                        Some(key) => key.unwrap(),
                        None => return Ok(()),
                    };

                    info!("read key: {:?}", key);

                    match key {
                        Key::Char('q') | Key::Ctrl('c') => break,
                        _ => (),
                    }
                }

                language_server_message = self.language_server_messages.next() => {
                    let (ctx, message) = match language_server_message {
                        Some((ctx, message)) => (ctx, message),
                        None => continue,
                    };

                    match message {
                        Message::Request(req) => {
                            if let Some(server) = self.ls_bridge.server(ctx).await {
                                info!("unknown request: {}", req.method);
                                server.respond(Response::method_not_found(req.id)).await?;
                            }
                        }
                        Message::Notification(not) => {
                            info!("unhandled notification: {:?}", not);
                        }
                        Message::Response(_) => panic!("responses should be handled in the lsp module"),
                    }
                }
            }
        }

        info!("terminating");

        Ok(())
    }

    /// Creates a language server context from the current workspace and buffer.
    ///
    /// Returns `None` if there is no active language for the current buffer.
    fn ls_context(&self) -> Option<lsp::Context> {
        let language_id = self.buffers.current().language_id?;

        Some(lsp::Context {
            root: self.current_dir.clone().to_uri(),
            language_id,
        })
    }

    async fn redraw(&self, term: &mut Terminal) -> Result<(), Error> {
        let bounds = Bounds::from_size(term.size());

        let mut ctx = ui::Context {
            bounds,
            screen: term.screen(),
        };

        let current_buffer = self.buffers.current();
        current_buffer.draw(&mut ctx);

        term.cursor = Coordinates::new(
            current_buffer.cursor.x as u16,
            current_buffer.cursor.y as u16,
        );

        term.refresh().await?;

        Ok(())
    }
}

/// Sets a panic hook that restores the terminal to its initial state and prints the panic message
/// to standard error.
///
/// Tokio swallows panics by default. We want to notify the user when a crash occurs, but we can't
/// just print the message because the alternate terminal screen is active during normal operation.
/// Furthermore, we want to restore the terminal to a usable state when the program exits. To
/// achieve both goals, we register a panic handler that restores the screen and exits raw mode
/// before printing the panic message to standard error.
///
/// Normally the destructors of `Stdin` and `Terminal` handle restoring the terminal for us, but
/// they run after this hook, so the panic message would be lost.
fn set_panic_hook(stdin: &Stdin, term: &Terminal) {
    // Termios is !Send, but we need to be able to send it to the panic hook.
    let old_termios = Arc::new(Mutex::new(stdin.old_termios.clone()));
    let restore_sequence = term.restore_sequence();

    panic::set_hook(Box::new(move |panic_info| {
        use std::io::Write;

        let backtrace = backtrace::Backtrace::new();
        error!("fatal error: {}\n{:?}", panic_info, backtrace);

        let mut stdout = std::io::stdout();
        let _ = stdout.write_all(&restore_sequence);

        if let Ok(termios) = old_termios.lock() {
            let stdin = std::io::stdin();
            let _ = termios::tcsetattr(stdin.as_raw_fd(), SetArg::TCSAFLUSH, &termios);
        }

        eprintln!("fatal error: {}", panic_info);
    }));
}
