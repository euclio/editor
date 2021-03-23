#![recursion_limit = "512"]
#![warn(clippy::dbg_macro)]
#![warn(clippy::print_stdout)]
#![warn(clippy::todo)]
#![cfg_attr(not(test), warn(clippy::unwrap_used))]

// workaround for rust-lang/rust#55779
extern crate serde;

use std::convert::TryFrom;
use std::env;
use std::os::unix::io::AsRawFd;
use std::panic;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::Error;
use futures::channel::mpsc;
use futures::{select, StreamExt};
use if_chain::if_chain;
use log::*;
use nix::sys::termios::{self, SetArg};
use structopt::StructOpt;
use tokio_stream::wrappers::SignalStream;

mod buffer;
mod config;
mod logger;
mod lsp;
mod syntax;
mod term;
mod ui;

use buffer::Buffers;
use config::Config;
use lsp::{LanguageServerBridge, Message, Response};
use term::{Key, Stdin, Terminal};
use tokio::signal::unix::{signal, SignalKind};
use ui::{Bounds, Coordinates, Drawable};

pub use logger::Logger;

/// Command-line options.
#[derive(Debug, StructOpt)]
pub struct Options {
    /// A list of filenames to edit.
    pub files: Vec<PathBuf>,
}

pub async fn run(options: Options) -> Result<(), Error> {
    let stdin = Stdin::new()?;
    let term = Terminal::new().await?;

    set_panic_hook(&stdin, &term);

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

    let (ls_tx, ls_rx) = mpsc::channel(10);

    let screen_size = term.size();
    let buffers =
        Buffers::from_paths(options.files.clone(), Bounds::from_size(screen_size)).await?;

    let mut editor = Editor {
        current_dir: env::current_dir()?,
        buffers,
        ls_bridge: LanguageServerBridge::new(language_server_config, ls_tx),
        language_server_messages: ls_rx,
        mode: Mode::Normal,
    };

    for buffer in &editor.buffers {
        if_chain! {
            if let Some(syntax) = buffer.syntax;
            if let Some(server) = editor.ls_bridge.get_or_init(editor.current_dir.clone(), lsp::Context { syntax }).await;
            if let Some(text_document_item) = buffer.to_text_document_item();
            then {
                server.did_open_text_document(text_document_item).await?;
            }
        }
    }

    editor.run(stdin, term).await
}

/// Core editor state.
pub struct Editor {
    current_dir: PathBuf,
    buffers: Buffers,
    ls_bridge: LanguageServerBridge,

    /// Receiver for requests and notifications from language servers.
    language_server_messages: mpsc::Receiver<(lsp::Context, lsp::Message)>,

    mode: Mode,
}

impl Editor {
    async fn run(mut self, stdin: Stdin, mut term: Terminal) -> Result<(), Error> {
        let mut stdin = stdin.fuse();
        let mut sigwinch = SignalStream::new(signal(SignalKind::window_change())?).fuse();

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

                    if let ControlFlow::Break = self.handle_key(key).await? {
                        break;
                    }
                }

                language_server_message = self.language_server_messages.next() => {
                    let (ctx, message) = match language_server_message {
                        Some((ctx, message)) => (ctx, message),
                        None => continue,
                    };

                    match message {
                        Message::Request(req) => {
                            if let Some(server) = self.ls_bridge.get(ctx) {
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

    /// Handles user-supplied key input.
    async fn handle_key(&mut self, key: Key) -> Result<ControlFlow, Error> {
        use Mode::*;

        match (self.mode, key) {
            (Normal, Key::Char('q')) => return Ok(ControlFlow::Break),
            (Normal, Key::Char('h')) => self.buffers.current_mut().move_left(),
            (Normal, Key::Char('i')) => self.mode = Insert,
            (Normal, Key::Char('j')) => self.buffers.current_mut().move_down(),
            (Normal, Key::Char('k')) => self.buffers.current_mut().move_up(),
            (Normal, Key::Char('l')) => self.buffers.current_mut().move_right(),
            (Insert, Key::Esc) => self.mode = Normal,
            (Insert, Key::Backspace) => self.delete_char().await?,
            (Insert, Key::Char(c)) => self.insert_char(c).await?,
            (Insert, Key::Return) => self.insert_char('\n').await?,
            _ => (),
        }

        Ok(ControlFlow::Continue)
    }

    async fn delete_char(&mut self) -> Result<(), Error> {
        let buffer = self.buffers.current_mut();
        let edit = buffer.delete();

        if_chain! {
            if let Some(edit) = edit;
            if let Some(syntax) = buffer.syntax;
            if let Some(versioned_identifier) = buffer.to_versioned_text_document_identifier();
            if let Some(server) = self.ls_bridge.get(lsp::Context { syntax });
            then {
                server.did_change_text_document(
                    versioned_identifier,
                    vec![edit.to_text_document_content_change_event()],
                ).await?;
            }
        }

        Ok(())
    }

    /// Insert a character into the active buffer.
    async fn insert_char(&mut self, c: char) -> Result<(), Error> {
        let buffer = self.buffers.current_mut();
        let edit = buffer.insert(c);

        if_chain! {
            if let Some(syntax) = buffer.syntax;
            if let Some(versioned_identifier) = buffer.to_versioned_text_document_identifier();
            if let Some(server) = self.ls_bridge.get(lsp::Context { syntax });
            then {
                server.did_change_text_document(
                    versioned_identifier,
                    vec![edit.to_text_document_content_change_event()],
                ).await?;
            }
        }

        Ok(())
    }

    async fn redraw(&self, term: &mut Terminal) -> Result<(), Error> {
        let bounds = Bounds::from_size(term.size());

        let mut ctx = ui::Context {
            bounds,
            screen: term.screen(),
        };

        ctx.screen.clear();

        let current_buffer = self.buffers.current();
        current_buffer.draw(&mut ctx);

        let cursor_position = current_buffer.cursor_position();
        term.cursor = Coordinates::new(
            u16::try_from(cursor_position.x).expect("cursor outside screen bounds"),
            u16::try_from(cursor_position.y).expect("cursor outside screen bounds"),
        );

        term.refresh().await?;

        Ok(())
    }
}

/// Editing mode.
#[derive(Debug, Copy, Clone)]
enum Mode {
    Normal,
    Insert,
}

impl Default for Mode {
    fn default() -> Self {
        Mode::Normal
    }
}

enum ControlFlow {
    Continue,
    Break,
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
