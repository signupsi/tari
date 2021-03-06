// Copyright 2019, The Tari Project
//
// Redistribution and use in source and binary forms, with or without modification, are permitted provided that the
// following conditions are met:
//
// 1. Redistributions of source code must retain the above copyright notice, this list of conditions and the following
// disclaimer.
//
// 2. Redistributions in binary form must reproduce the above copyright notice, this list of conditions and the
// following disclaimer in the documentation and/or other materials provided with the distribution.
//
// 3. Neither the name of the copyright holder nor the names of its contributors may be used to endorse or promote
// products derived from this software without specific prior written permission.
//
// THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES,
// INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
// DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
// SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
// SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY,
// WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE
// USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

use super::{error::ConnectionManagerError, peer_connection::PeerConnection};
use crate::peer_manager::NodeId;
use futures::{
    channel::{mpsc, oneshot},
    SinkExt,
};

/// Requests which are handled by the ConnectionManagerService
pub enum ConnectionManagerRequest {
    DialPeer(NodeId, oneshot::Sender<Result<PeerConnection, ConnectionManagerError>>),
}

/// Responsible for constructing requests to the ConnectionManagerService
#[derive(Clone)]
pub struct ConnectionManagerRequester {
    sender: mpsc::Sender<ConnectionManagerRequest>,
}

impl ConnectionManagerRequester {
    /// Create a new ConnectionManagerRequester
    pub fn new(sender: mpsc::Sender<ConnectionManagerRequest>) -> Self {
        Self { sender }
    }
}

impl ConnectionManagerRequester {
    /// Attempt to connect to a remote peer
    pub async fn dial_peer(&mut self, node_id: NodeId) -> Result<PeerConnection, ConnectionManagerError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.sender
            .send(ConnectionManagerRequest::DialPeer(node_id, reply_tx))
            .await
            .map_err(|_| ConnectionManagerError::SendToActorFailed)?;
        reply_rx
            .await
            .map_err(|_| ConnectionManagerError::ActorRequestCanceled)?
    }
}
