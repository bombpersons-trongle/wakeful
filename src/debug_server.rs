//! The debug MCP server: the running game exposes tools on localhost
//! so an agent can drive, inspect, and script it. Screenshots come
//! back as images, state as text, and `eval` runs rhai against the
//! live world-tier environment (shared stores, UI, battle readers, the
//! party, scene operations — plus the `lib/` shared code).
//!
//! Commands flow into [`DebugCommands`], drained each fixed tick by
//! `debug_shot::serve_debug_commands`, which completes every command's
//! reply channel. Gated on `cfg(debug_assertions)` like the rest of
//! the debug layer.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use rmcp::handler::server::{router::tool::ToolRouter, wrapper::Parameters};
use rmcp::model::{
    CallToolResult, ContentBlock as Content, Implementation, ServerCapabilities, ServerConfig,
};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService,
};
use rmcp::{tool, tool_handler, tool_router, ErrorData as McpError, ServerHandler};

/// How long a tool call waits for the game to service it. The game
/// services at the next fixed tick (≤17 ms), so this only trips if the
/// game is wedged.
const REPLY_TIMEOUT: Duration = Duration::from_secs(2);

/// The shared queue: the server pushes commands, the game drains.
#[derive(Clone, Default, bevy::prelude::Resource)]
pub(crate) struct DebugCommands(Arc<Mutex<Vec<DebugCommand>>>);

impl DebugCommands {
    pub(crate) fn push(&self, command: DebugCommand) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(command);
    }

    /// Takes everything queued, leaving the channel empty.
    pub(crate) fn take(&self) -> Vec<DebugCommand> {
        std::mem::take(
            &mut self
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }
}

/// One tool call waiting for the game.
pub(crate) struct DebugCommand {
    pub(crate) kind: DebugKind,
    pub(crate) reply: std::sync::mpsc::SyncSender<Result<DebugReply, String>>,
}

pub(crate) enum DebugKind {
    Screenshot,
    Tap(String),
    Hold(String),
    Release(String),
    State,
    Eval(String),
}

/// Tool input: one action name (object root per the MCP schema).
#[derive(serde::Deserialize, rmcp::schemars::JsonSchema)]
pub struct ActionArgs {
    pub action: String,
}

/// Tool input: a rhai snippet.
#[derive(serde::Deserialize, rmcp::schemars::JsonSchema)]
pub struct CodeArgs {
    pub code: String,
}

#[derive(std::fmt::Debug)]
pub(crate) enum DebugReply {
    /// A PNG-encoded screenshot.
    Png(Vec<u8>),
    Text(String),
}

/// The MCP tool surface. Stateless: every call walks the queue.
#[derive(Clone)]
pub(crate) struct WakefulTools {
    commands: DebugCommands,
    // Read by the #[tool_handler] expansion; invisible to clippy.
    #[allow(dead_code)]
    tool_router: ToolRouter<WakefulTools>,
}

/// Screenshot tool arguments.


#[tool_router]
impl WakefulTools {
    pub(crate) fn new(commands: DebugCommands) -> Self {
        Self {
            commands,
            tool_router: Self::tool_router(),
        }
    }

    fn send(&self, kind: DebugKind) -> Result<DebugReply, McpError> {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        self.commands.push(DebugCommand { kind, reply: tx });
        match rx.recv_timeout(REPLY_TIMEOUT) {
            Ok(Ok(reply)) => Ok(reply),
            Ok(Err(message)) => Err(McpError::internal_error(message, None)),
            Err(_) => Err(McpError::internal_error(
                "the game did not answer in time (is it wedged?)",
                None,
            )),
        }
    }

    fn text(&self, kind: DebugKind) -> Result<CallToolResult, McpError> {
        match self.send(kind)? {
            DebugReply::Text(text) => Ok(CallToolResult::success(vec![Content::text(text)])),
            DebugReply::Png(_) => Err(McpError::internal_error(
                "unexpected screenshot reply".to_owned(),
                None,
            )),
        }
    }

    fn press(&self, kind: DebugKind, action: String) -> Result<CallToolResult, McpError> {
        self.text(kind)
            .map(|_| CallToolResult::success(vec![Content::text(format!("{action} ok"))]))
    }

    #[tool(description = "Capture the game frame as a PNG image.")]
    fn screenshot(&self) -> Result<CallToolResult, McpError> {
        match self.send(DebugKind::Screenshot)? {
            DebugReply::Png(bytes) => {
                let encoded = {
                    use base64::Engine as _;
                    base64::engine::general_purpose::STANDARD.encode(bytes)
                };
                Ok(CallToolResult::success(vec![Content::image(
                    encoded,
                    "image/png",
                )]))
            }
            DebugReply::Text(error) => Err(McpError::internal_error(error, None)),
        }
    }

    #[tool(description = "Press and release a control for one tick. Actions are the configured pad names: cross, circle, triangle, square, dpad_up/down/left/right, l1..r3, start, select.")]
    fn tap(
        &self,
        Parameters(ActionArgs { action }): Parameters<ActionArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.press(DebugKind::Tap(action.clone()), action)
    }

    #[tool(description = "Hold a control down until a matching release. Same action names as tap.")]
    fn hold(
        &self,
        Parameters(ActionArgs { action }): Parameters<ActionArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.press(DebugKind::Hold(action.clone()), action)
    }

    #[tool(description = "Release a control previously held.")]
    fn release(
        &self,
        Parameters(ActionArgs { action }): Parameters<ActionArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.press(DebugKind::Release(action.clone()), action)
    }

    #[tool(description = "Dump the live debug state: world stores, battle participants, cameras, actors, animation drivers.")]
    fn state(&self) -> Result<CallToolResult, McpError> {
        self.text(DebugKind::State)
    }

    #[tool(description = "Run rhai against the live game: the shared global store (remember_global/recall_global), UI, battle reads/writes, party, scene ops (warp_to, teleport_player), and the lib/ helpers (r::give_xp, floats::float_text). Multi-statement snippets are fine; the last expression is the result.")]
    fn eval(
        &self,
        Parameters(CodeArgs { code }): Parameters<CodeArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.text(DebugKind::Eval(code))
    }
}

#[tool_handler]
impl ServerHandler for WakefulTools {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::from_build_env())
            .with_instructions(
                "wakeful's debug surface: drive the game, capture frames, and script it live.",
            )
    }
}

/// True when the interactive stdin console was requested: debug
/// builds running with `--console`.
pub(crate) fn console_requested() -> bool {
    cfg!(debug_assertions) && std::env::args().any(|arg| arg == "--console")
}

/// Lines the REPL hands to the evaluator: blanks and comment-only
/// lines are skipped.
pub(crate) fn repl_skips(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.is_empty() || trimmed.starts_with("//")
}

/// An interactive rhai REPL on stdin: each line runs through the same
/// executor as the MCP `eval` tool, results and errors print inline.
/// Ends quietly at EOF (piped or nohup launches simply have no
/// console). Blocks its own thread.
pub(crate) fn spawn_repl(commands: bevy::prelude::Res<DebugCommands>) {
    let commands = DebugCommands::clone(&commands);
    let result = std::thread::Builder::new()
        .name("debug-repl".into())
        .spawn(move || {
            use std::io::{BufRead as _, Write as _};
            let stdin = std::io::stdin();
            println!("wakeful console: rhai lines, ctrl-d exits");
            loop {
                print!("rhai> ");
                std::io::stdout().flush().ok();
                let Some(Ok(line)) = stdin.lock().lines().next() else {
                    break;
                };
                if repl_skips(&line) {
                    continue;
                }
                let (tx, rx) = std::sync::mpsc::sync_channel(1);
                commands.push(DebugCommand {
                    kind: DebugKind::Eval(line),
                    reply: tx,
                });
                match rx.recv_timeout(REPLY_TIMEOUT) {
                    Ok(Ok(DebugReply::Text(text))) => println!("{text}"),
                    Ok(Ok(_)) => println!("(no value)"),
                    Ok(Err(message)) => println!("error: {message}"),
                    Err(_) => println!("error: the game did not answer in time"),
                }
            }
        });
    if let Err(e) = result {
        bevy::log::warn!("debug console could not spawn: {e}");
    }
}

/// Binds and serves, blocking its own thread. Called once at startup
/// (debug builds); a busy port means another instance owns it and this
/// one skips the server quietly.
pub(crate) fn spawn_server(commands: bevy::prelude::Res<DebugCommands>) {
    let commands = commands.clone();
    let port = std::env::var("WAKEFUL_MCP_PORT")
        .ok()
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(8399);
    let result = std::thread::Builder::new()
        .name("debug-mcp".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("the debug server's tokio runtime");
            runtime.block_on(async move {
                let service = StreamableHttpService::new(
                    move || Ok(WakefulTools::new(commands.clone())),
                    Arc::new(LocalSessionManager::default()),
                    StreamableHttpServerConfig::default(),
                );
                let router = axum::Router::new().nest_service("/mcp", service);
                let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
                    .await
                    .expect("bind the debug MCP port");
                bevy::log::info!("debug MCP server on http://127.0.0.1:{port}/mcp");
                axum::serve(listener, router).await.expect("debug server");
            });
        });
    if let Err(e) = result {
        bevy::log::warn!("debug MCP server could not spawn: {e}");
    }
}
