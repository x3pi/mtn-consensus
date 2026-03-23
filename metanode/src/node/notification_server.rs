// Copyright (c) MetaNode Team
// SPDX-License-Identifier: Apache-2.0

use crate::node::executor_client::proto::{
    request, response, NotifyEpochChangeResponse, Request, Response,
};
use anyhow::Result;
use futures::{SinkExt, StreamExt};
use prost::Message;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::{UnixListener, UnixStream};
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use tracing::{error, info, warn};

/// Server that listens for notifications from the external executor (Go)
/// This enables event-driven architecture where Go PUSHES events to Rust
/// instead of Rust polling Go.
pub struct EpochNotificationServer {
    socket_path: PathBuf,
    epoch_transition_callback: Arc<dyn Fn(u64, u64, u64) -> Result<()> + Send + Sync>,
}

impl EpochNotificationServer {
    pub fn new(
        socket_path: PathBuf,
        epoch_transition_callback: impl Fn(u64, u64, u64) -> Result<()> + Send + Sync + 'static,
    ) -> Self {
        Self {
            socket_path,
            epoch_transition_callback: Arc::new(epoch_transition_callback),
        }
    }

    pub async fn start(&self) -> Result<()> {
        // Remove existing socket if present
        if self.socket_path.exists() {
            std::fs::remove_file(&self.socket_path)?;
        }

        let listener = UnixListener::bind(&self.socket_path)?;

        // Restrict socket permissions to owner + group only (same as tx_socket_server)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o660);
            std::fs::set_permissions(&self.socket_path, perms)?;
        }

        info!(
            "👂 [NOTIFICATION SERVER] Listening on {}",
            self.socket_path.display()
        );

        let callback = self.epoch_transition_callback.clone();

        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, _addr)) => {
                        let callback_clone = callback.clone();
                        tokio::spawn(async move {
                            if let Err(e) = Self::handle_connection(stream, callback_clone).await {
                                warn!("⚠️ [NOTIFICATION SERVER] Connection error: {}", e);
                            }
                        });
                    }
                    Err(e) => {
                        error!("❌ [NOTIFICATION SERVER] Accept error: {}", e);
                        // Sleep to avoid busy loop on permanent error
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    }
                }
            }
        });

        Ok(())
    }

    async fn handle_connection(
        stream: UnixStream,
        callback: Arc<dyn Fn(u64, u64, u64) -> Result<()> + Send + Sync>,
    ) -> Result<()> {
        let mut framed = Framed::new(stream, LengthDelimitedCodec::new());

        while let Some(msg_res) = framed.next().await {
            let msg_bytes = msg_res?;
            let request = Request::decode(msg_bytes.as_ref())?;

            let response = match request.payload {
                Some(request::Payload::NotifyEpochChangeRequest(req)) => {
                    info!(
                        "📣 [NOTIFICATION] Received Epoch Change Notification: epoch {} -> {}, boundary={}",
                        // "epoch" here implies current/old epoch? The request has new_epoch.
                        // We'll just log new_epoch.
                        "?", req.new_epoch, req.boundary_block
                    );

                    match callback(req.new_epoch, req.epoch_timestamp_ms, req.boundary_block) {
                        Ok(_) => Response {
                            payload: Some(response::Payload::NotifyEpochChangeResponse(
                                NotifyEpochChangeResponse {
                                    success: true,
                                    message: "Transition triggered".to_string(),
                                },
                            )),
                        },
                        Err(e) => {
                            error!("❌ [NOTIFICATION] Callback failed: {}", e);
                            Response {
                                payload: Some(response::Payload::NotifyEpochChangeResponse(
                                    NotifyEpochChangeResponse {
                                        success: false,
                                        message: format!("Callback failed: {}", e),
                                    },
                                )),
                            }
                        }
                    }
                }
                Some(_) => {
                    warn!("⚠️ [NOTIFICATION] Received unexpected request type");
                    Response {
                        payload: Some(response::Payload::Error(
                            "Unexpected request type".to_string(),
                        )),
                    }
                }
                None => Response {
                    payload: Some(response::Payload::Error("Empty payload".to_string())),
                },
            };

            let mut resp_bytes = Vec::new();
            response.encode(&mut resp_bytes)?;
            framed.send(resp_bytes.into()).await?;
        }

        Ok(())
    }
}
