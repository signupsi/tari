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

use crate::{actor::DhtRequester, inbound::DhtInboundMessage};
use futures::{task::Context, Future, Poll};
use log::*;
use tari_comms_middleware::MiddlewareError;
use tower::{layer::Layer, Service, ServiceExt};

const LOG_TARGET: &'static str = "comms::dht::dedup";

/// # DHT Deduplication middleware
///
/// Takes in a `DhtInboundMessage` and checks the message signature cache for duplicates.
/// If a duplicate message is detected, it is discarded.
#[derive(Clone)]
pub struct DedupMiddleware<S> {
    next_service: S,
    dht_requester: DhtRequester,
}

impl<S> DedupMiddleware<S> {
    pub fn new(service: S, dht_requester: DhtRequester) -> Self {
        Self {
            next_service: service,
            dht_requester,
        }
    }
}

impl<S> Service<DhtInboundMessage> for DedupMiddleware<S>
where
    S: Service<DhtInboundMessage, Response = ()> + Clone + 'static,
    S::Error: Into<MiddlewareError>,
{
    type Error = MiddlewareError;
    type Response = ();

    type Future = impl Future<Output = Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, msg: DhtInboundMessage) -> Self::Future {
        Self::process_message(self.next_service.clone(), self.dht_requester.clone(), msg)
    }
}

impl<S> DedupMiddleware<S>
where
    S: Service<DhtInboundMessage, Response = ()>,
    S::Error: Into<MiddlewareError>,
{
    pub async fn process_message(
        next_service: S,
        mut dht_requester: DhtRequester,
        message: DhtInboundMessage,
    ) -> Result<(), MiddlewareError>
    {
        trace!(target: LOG_TARGET, "Checking inbound message cache for duplicates");
        // WARN: It is assumed that the message signature has been checked (i.e. by the DeserializeMiddleware)
        let signature = message.dht_header.origin_signature.clone();
        if dht_requester.insert_message_signature(signature).await? {
            warn!(
                target: LOG_TARGET,
                "Received duplicate message from peer {} (source={}). Message discarded.",
                message.source_peer.node_id,
                message.dht_header.origin_public_key
            );
            return Ok(());
        }
        next_service.oneshot(message).await.map_err(Into::into)
    }
}

pub struct DedupLayer {
    dht_requester: DhtRequester,
}

impl DedupLayer {
    pub fn new(dht_requester: DhtRequester) -> Self {
        Self { dht_requester }
    }
}

impl<S> Layer<S> for DedupLayer {
    type Service = DedupMiddleware<S>;

    fn layer(&self, service: S) -> Self::Service {
        DedupMiddleware::new(service, self.dht_requester.clone())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::{
        envelope::DhtMessageFlags,
        test_utils::{create_dht_actor_mock, make_dht_inbound_message, make_node_identity, service_spy, DhtMockState},
    };
    use tari_test_utils::panic_context;
    use tokio::runtime::Runtime;

    #[test]
    fn process_message() {
        let rt = Runtime::new().unwrap();
        let spy = service_spy();

        let (dht_requester, mut mock) = create_dht_actor_mock(1);
        let mock_state = DhtMockState::new();
        mock_state.set_signature_cache_insert(false);
        mock.set_shared_state(mock_state.clone());
        rt.spawn(mock.run());

        let mut dedup = DedupLayer::new(dht_requester).layer(spy.to_service::<MiddlewareError>());

        panic_context!(cx);

        assert!(dedup.poll_ready(&mut cx).is_ready());
        let node_identity = make_node_identity();
        let msg = make_dht_inbound_message(&node_identity, Vec::new(), DhtMessageFlags::empty());

        rt.block_on(dedup.call(msg.clone())).unwrap();
        assert_eq!(spy.call_count(), 1);

        mock_state.set_signature_cache_insert(true);
        rt.block_on(dedup.call(msg)).unwrap();
        assert_eq!(spy.call_count(), 1);
        // Drop dedup so that the DhtMock will stop running
        drop(dedup);
        rt.shutdown_on_idle();
    }
}
