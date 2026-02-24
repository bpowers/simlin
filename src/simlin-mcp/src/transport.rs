// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Async transport layer for the MCP server.
//!
//! Decouples the protocol dispatch loop from the actual I/O medium.
//! The `StdioTransport` implementation spawns three cooperating tokio tasks:
//! a stdin reader, a stdout writer, and holds the channel endpoints that
//! connect them to the protocol dispatcher.

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

// ── Transport trait ──────────────────────────────────────────────────

/// Async message transport used by the MCP protocol dispatcher.
///
/// `recv` returns `None` when the transport is closed (e.g. stdin EOF),
/// which signals the server loop to shut down cleanly.
pub trait Transport {
    async fn recv(&mut self) -> Option<String>;
    async fn send(&self, message: String) -> anyhow::Result<()>;
}

// ── StdioTransport ───────────────────────────────────────────────────

/// Three-task stdio transport.
///
/// Spawns a stdin-reader task (bounded channel, capacity 128) and a
/// stdout-writer task (unbounded channel).  The transport struct holds
/// the receiver end of the stdin channel and the sender end of the stdout
/// channel, so the protocol dispatcher can call `recv`/`send` without
/// touching raw I/O directly.
///
/// The writer task's `JoinHandle` is stored so that `shutdown()` can wait
/// for all queued responses to be flushed before the process exits.
pub struct StdioTransport {
    stdin_rx: mpsc::Receiver<String>,
    stdout_tx: mpsc::UnboundedSender<String>,
    writer_handle: tokio::task::JoinHandle<()>,
}

impl StdioTransport {
    pub fn new() -> Self {
        let (stdin_tx, stdin_rx) = mpsc::channel::<String>(128);
        let (stdout_tx, mut stdout_rx) = mpsc::unbounded_channel::<String>();

        // Stdin reader task: reads newline-delimited JSON from stdin and
        // forwards non-empty lines to the protocol dispatcher via stdin_tx.
        // Dropping stdin_tx when stdin reaches EOF signals shutdown.
        tokio::spawn(async move {
            let stdin = tokio::io::stdin();
            let mut reader = BufReader::new(stdin);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) => break, // EOF
                    Ok(_) => {
                        let trimmed = line.trim().to_string();
                        if !trimmed.is_empty() && stdin_tx.send(trimmed).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        // Stdout writer task: receives serialized responses and writes them
        // as newline-terminated lines to stdout, flushing after each write.
        // The task exits when stdout_rx is closed (i.e., when stdout_tx is dropped).
        let writer_handle = tokio::spawn(async move {
            let mut stdout = tokio::io::stdout();
            while let Some(msg) = stdout_rx.recv().await {
                let line = format!("{msg}\n");
                if stdout.write_all(line.as_bytes()).await.is_err() {
                    break;
                }
                if stdout.flush().await.is_err() {
                    break;
                }
            }
        });

        Self {
            stdin_rx,
            stdout_tx,
            writer_handle,
        }
    }

    /// Signal the writer task that no more messages are coming and wait for
    /// it to finish flushing all queued responses.
    ///
    /// Must be called after `serve_async` returns to ensure that responses
    /// queued just before EOF are written before the process exits.
    pub async fn shutdown(self) {
        drop(self.stdout_tx);
        let _ = self.writer_handle.await;
    }
}

impl Transport for StdioTransport {
    async fn recv(&mut self) -> Option<String> {
        self.stdin_rx.recv().await
    }

    async fn send(&self, message: String) -> anyhow::Result<()> {
        self.stdout_tx
            .send(message)
            .map_err(|e| anyhow::anyhow!("stdout channel closed: {e}"))
    }
}
