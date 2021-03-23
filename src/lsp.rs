//! Language server communication and management.

use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::env;
use std::num::Wrapping;
use std::path::{Path, PathBuf};
use std::process::{self, Stdio};
use std::sync::Arc;

use futures::channel::{mpsc, oneshot};
use futures::lock::Mutex;
use futures::{future, SinkExt, TryStreamExt};
use log::*;
use lsp_types::notification::{
    DidChangeTextDocument, DidOpenTextDocument, Initialized, Notification as LspTypesNotification,
};
use lsp_types::request::{Initialize, Request as LspTypesRequest};
use lsp_types::{
    ClientCapabilities, ClientInfo, DidChangeTextDocumentParams, DidOpenTextDocumentParams,
    InitializeParams, InitializeResult, InitializedParams, ServerInfo,
    TextDocumentContentChangeEvent, TextDocumentItem, VersionedTextDocumentIdentifier,
};
use serde::Deserialize;
use thiserror::Error;
use tokio::io::{self, AsyncBufReadExt, BufReader};
use tokio::process::{ChildStdin, Command};
use tokio_stream::wrappers::LinesStream;
use tokio_util::codec::{FramedRead, FramedWrite};

use crate::config::LanguageServerConfig;
use crate::syntax::Syntax;

mod protocol;

use protocol::{Id, LspCodec, ResponseError};

pub use protocol::{Message, Notification, Request, Response};

pub type Uri = lsp_types::Url;

pub type Result<T> = std::result::Result<T, Error>;

/// Context to identify a particular language server.
#[derive(Debug, Clone)]
pub struct Context {
    /// The hosted language.
    pub syntax: Syntax,
    // TODO: Split into client/server context and add server name?
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("LSP I/O error: {0}")]
    Io(#[from] io::Error),

    #[error("LSP server hung up unexpectedly")]
    Canceled(#[from] oneshot::Canceled),

    #[error("bad response from LSP server: {0}")]
    ResponseError(#[from] ResponseError),

    #[error("could not deserialize LSP response: {0}")]
    DeserializationError(#[from] serde_json::Error),
}

/// Manages language servers.
pub struct LanguageServerBridge {
    config: HashMap<Syntax, LanguageServerConfig>,

    language_to_server: HashMap<Syntax, LanguageServer>,

    /// Cloneable sender for language server requests and notifications.
    server_sender: mpsc::Sender<(Context, Message)>,
}

impl LanguageServerBridge {
    pub fn new(
        config: HashMap<Syntax, LanguageServerConfig>,
        server_sender: mpsc::Sender<(Context, Message)>,
    ) -> Self {
        LanguageServerBridge {
            config,
            language_to_server: HashMap::new(),
            server_sender,
        }
    }

    pub fn get(&mut self, ctx: Context) -> Option<&mut LanguageServer> {
        self.language_to_server.get_mut(&ctx.syntax)
    }

    pub async fn get_or_init(
        &mut self,
        root: PathBuf,
        ctx: Context,
    ) -> Option<&mut LanguageServer> {
        match self.language_to_server.entry(ctx.syntax) {
            Entry::Occupied(entry) => Some(entry.into_mut()),
            Entry::Vacant(entry) => {
                let (prog, args) = self.config.get(&ctx.syntax)?.command();
                let mut command = Command::new(prog);
                command.args(args);

                let server_sender = self.server_sender.clone();
                let mut server =
                    match LanguageServer::spawn(command, ctx.clone(), server_sender).await {
                        Ok(server) => server,
                        Err(err) => {
                            error!("unable to start language server: {}", err);
                            return None;
                        }
                    };

                let initialize_result = match server.initialize(root.to_uri()).await {
                    Ok(result) => result,
                    Err(e) => {
                        info!("unable to initialize {}: {}", prog, e);
                        return None;
                    }
                };
                info!(
                    "successfully initialized {}",
                    match initialize_result.server_info {
                        Some(ServerInfo {
                            name,
                            version: Some(version),
                        }) => format!("{} {}", name, version),
                        Some(ServerInfo {
                            name,
                            version: None,
                        }) => name,
                        None => String::from(prog),
                    },
                );
                server.initialized().await.ok()?;

                Some(entry.insert(server))
            }
        }
    }
}

pub struct LanguageServer {
    next_request_id: Wrapping<u64>,
    pending_responses: Arc<Mutex<HashMap<Id, oneshot::Sender<protocol::Response>>>>,
    stdin: FramedWrite<ChildStdin, LspCodec>,
}

impl LanguageServer {
    async fn spawn(
        mut command: Command,
        context: Context,
        message_sender: mpsc::Sender<(Context, Message)>,
    ) -> io::Result<Self> {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env_remove("RUST_LOG")
            .spawn()?;

        let stdin = child.stdin.take().expect("stdin was not piped");
        let stdout = child.stdout.take().expect("stdout was not piped");
        let stderr = child.stderr.take().expect("stderr was not piped");

        tokio::spawn(async {
            LinesStream::new(BufReader::new(stderr).lines())
                .try_for_each(|line| {
                    info!("stderr: {}", line);
                    future::ready(Ok(()))
                })
                .await
                .expect("error reading stderr from server");
        });

        // TODO: Should be able to remove these Arc/Mutexes, we're using the single-threaded runtime.
        let pending_responses = Arc::new(Mutex::new(HashMap::new()));
        let server_pending_responses = Arc::clone(&pending_responses);
        let server_message_sender = Arc::new(Mutex::new(message_sender));

        // let server_request_sender = self.server_request_sender.clone();
        tokio::spawn(async move {
            let stdout = FramedRead::new(stdout, LspCodec);
            let ctx = context;
            stdout
                .try_for_each(|message| async {
                    let message_sender = server_message_sender.clone();

                    match message {
                        Message::Response(response) => {
                            if let Some(id) = &response.id {
                                let sender: Option<oneshot::Sender<_>> =
                                    server_pending_responses.lock().await.remove(id);

                                match sender {
                                    Some(sender) => sender
                                        .send(response)
                                        .expect("unable to send response from server"),
                                    None => warn!(
                                        "received response for non-existent request id: {}",
                                        id
                                    ),
                                }
                            }
                        }
                        Message::Request(_) | Message::Notification(_) => {
                            message_sender
                                .lock()
                                .await
                                .send((ctx.clone(), message))
                                .await
                                .expect("unable to send request or notification from server");
                        }
                    }

                    Ok(())
                })
                .await
                .expect("unable to decode language server stdout");
        });

        Ok(LanguageServer {
            next_request_id: Wrapping(0),
            pending_responses,
            stdin: FramedWrite::new(stdin, LspCodec),
        })
    }

    pub async fn respond(&mut self, response: Response) -> Result<()> {
        self.stdin.send(Message::Response(response)).await?;
        Ok(())
    }

    pub async fn did_open_text_document(&mut self, text_document: TextDocumentItem) -> Result<()> {
        self.notify::<DidOpenTextDocument>(DidOpenTextDocumentParams { text_document })
            .await
    }

    pub async fn did_change_text_document(
        &mut self,
        text_document: VersionedTextDocumentIdentifier,
        content_changes: Vec<TextDocumentContentChangeEvent>,
    ) -> Result<()> {
        self.notify::<DidChangeTextDocument>(DidChangeTextDocumentParams {
            text_document,
            content_changes,
        })
        .await
    }

    async fn request<Req: LspTypesRequest>(&mut self, params: Req::Params) -> Result<Req::Result> {
        let id = self.next_request_id();

        let (response_tx, response_rx) = oneshot::channel();
        self.pending_responses
            .lock()
            .await
            .insert(id.clone(), response_tx);

        let req = Message::request::<Req>(id, params);
        self.stdin.send(req).await?;

        let res = response_rx.await?.result?;
        <_>::deserialize(res).map_err(Into::into)
    }

    async fn notify<N: LspTypesNotification>(&mut self, params: N::Params) -> Result<()> {
        self.stdin
            .send(Message::notification::<N>(params))
            .await
            .map_err(Into::into)
    }

    async fn initialize(&mut self, root_uri: Uri) -> Result<InitializeResult> {
        #[allow(deprecated)]
        let params = InitializeParams {
            process_id: Some(process::id().into()),
            client_info: Some(client_info()),
            root_path: None,
            root_uri: Some(root_uri),
            initialization_options: None,
            capabilities: client_capabilities(),
            trace: None,
            workspace_folders: None,
        };

        self.request::<Initialize>(params).await
    }

    async fn initialized(&mut self) -> Result<()> {
        self.notify::<Initialized>(InitializedParams {}).await
    }

    fn next_request_id(&mut self) -> Id {
        let id = Id::from(self.next_request_id.0);
        self.next_request_id += Wrapping(1);
        id
    }
}

pub trait ToUri {
    fn to_uri(&self) -> Uri;
}

impl<P: AsRef<Path>> ToUri for P {
    fn to_uri(&self) -> Uri {
        let path = self.as_ref();
        Uri::from_file_path(path)
            .map_err(|()| format!("{} is not an absolute path", path.display()))
            .expect("could not convert path to URI")
    }
}

fn client_info() -> ClientInfo {
    ClientInfo {
        name: String::from(env!("CARGO_PKG_NAME")),
        version: Some(String::from(env!("CARGO_PKG_VERSION"))),
    }
}

fn client_capabilities() -> ClientCapabilities {
    ClientCapabilities::default()
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::path::PathBuf;

    use super::{ToUri, Uri};

    #[test]
    fn path_to_uri() -> Result<(), Box<dyn Error>> {
        let uri = PathBuf::from("/foo/bar/baz").to_uri();
        assert_eq!(uri, Uri::parse("file:///foo/bar/baz")?);
        Ok(())
    }

    #[test]
    fn path_to_uri_needs_escaping() -> Result<(), Box<dyn Error>> {
        let uri = PathBuf::from("/encode#me?").to_uri();
        assert_eq!(uri, Uri::parse("file:///encode%23me%3F")?);
        Ok(())
    }

    #[test]
    fn path_to_uri_with_spaces() -> Result<(), Box<dyn Error>> {
        let uri = PathBuf::from("/path with spaces/test").to_uri();
        assert_eq!(uri, Uri::parse("file:///path%20with%20spaces/test")?);
        Ok(())
    }

    #[test]
    #[should_panic = "src/main.rs is not an absolute path"]
    fn relative_path_to_uri_panics() {
        PathBuf::from("src/main.rs").to_uri();
    }
}
