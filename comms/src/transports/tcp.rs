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

use super::Transport;
use crate::utils::multiaddr::{multiaddr_to_socketaddr, socketaddr_to_multiaddr};
use futures::{future, io::Error, ready, stream::BoxStream, AsyncRead, AsyncWrite, Future, Poll, Stream, StreamExt};
use multiaddr::Multiaddr;
use std::{io, pin::Pin, task::Context, time::Duration};
use tokio::{
    io::{AsyncRead as TokioAsyncRead, AsyncWrite as TokioAsyncWrite},
    net::{TcpListener, TcpStream},
};

/// Transport implementation for TCP
#[derive(Debug, Clone, Default)]
pub struct TcpTransport {
    recv_buffer_size: Option<usize>,
    send_buffer_size: Option<usize>,
    ttl: Option<u32>,
    keepalive: Option<Option<Duration>>,
    nodelay: Option<bool>,
}

impl TcpTransport {
    /// Sets `SO_RCVBUF` i.e the size of the receive buffer.
    setter_mut!(set_recv_buffer_size, recv_buffer_size, Option<usize>);

    /// Sets `SO_SNDBUF` i.e. the size of the send buffer.
    setter_mut!(set_send_buffer_size, send_buffer_size, Option<usize>);

    /// Sets `IP_TTL` i.e. the TTL of packets sent from this socket.
    setter_mut!(set_ttl, ttl, Option<u32>);

    /// Sets `SO_KEEPALIVE` i.e. the interval to send keepalive probes, or None to disable.
    setter_mut!(set_keepalive, keepalive, Option<Option<Duration>>);

    /// Sets `TCP_NODELAY` i.e enable/disable Nagle's algorithm.
    setter_mut!(set_nodelay, nodelay, Option<bool>);

    /// Create a new TcpTransport
    pub fn new() -> Self {
        Default::default()
    }

    /// Apply socket options to `TcpStream`.
    fn configure(&self, socket: &TcpStream) -> io::Result<()> {
        if let Some(keepalive) = self.keepalive {
            socket.set_keepalive(keepalive)?;
        }

        if let Some(ttl) = self.ttl {
            socket.set_ttl(ttl)?;
        }

        if let Some(nodelay) = self.nodelay {
            socket.set_nodelay(nodelay)?;
        }

        if let Some(recv_buffer_size) = self.recv_buffer_size {
            socket.set_recv_buffer_size(recv_buffer_size)?;
        }

        if let Some(send_buffer_size) = self.send_buffer_size {
            socket.set_send_buffer_size(send_buffer_size)?;
        }

        Ok(())
    }
}

impl Transport for TcpTransport {
    type Error = io::Error;
    type Inbound = future::Ready<io::Result<(TcpSocket, Multiaddr)>>;
    type Listener = TcpInbound<'static>;
    type Output = (TcpSocket, Multiaddr);

    type DialFuture = impl Future<Output = io::Result<Self::Output>>;
    type ListenFuture = impl Future<Output = io::Result<(Self::Listener, Multiaddr)>>;

    fn listen(&self, addr: Multiaddr) -> Self::ListenFuture {
        let config = self.clone();
        Box::pin(async move {
            let socket_addr = multiaddr_to_socketaddr(&addr)?;
            let listener = TcpListener::bind(&socket_addr).await?;
            let local_addr = socketaddr_to_multiaddr(&listener.local_addr()?);
            Ok((
                TcpInbound {
                    incoming: listener.incoming().boxed(),
                    config,
                },
                local_addr,
            ))
        })
    }

    fn dial(&self, addr: Multiaddr) -> Self::DialFuture {
        let config = self.clone();
        Box::pin(async move {
            let socket_addr = multiaddr_to_socketaddr(&addr)?;
            let stream = TcpStream::connect(&socket_addr).await?;
            config.configure(&stream)?;
            let peer_addr = socketaddr_to_multiaddr(&stream.peer_addr()?);
            Ok((TcpSocket::new(stream), peer_addr))
        })
    }
}

/// Wrapper around an Inbound stream. This ensures that any connecting `TcpStream` is configured according to the
/// transport
pub struct TcpInbound<'a> {
    incoming: BoxStream<'a, io::Result<TcpStream>>,
    config: TcpTransport,
}

impl Stream for TcpInbound<'_> {
    type Item = io::Result<future::Ready<io::Result<(TcpSocket, Multiaddr)>>>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match ready!(self.incoming.poll_next_unpin(cx)) {
            Some(Ok(stream)) => {
                // Configure each socket
                self.config.configure(&stream)?;
                let peer_addr = socketaddr_to_multiaddr(&stream.peer_addr()?);
                let result = future::ready(Ok((TcpSocket::new(stream), peer_addr)));
                Poll::Ready(Some(Ok(result)))
            },
            Some(Err(err)) => Poll::Ready(Some(Err(err))),
            None => Poll::Ready(None),
        }
    }
}

/// TcpSocket is a wrapper struct for tokio `TcpStream` and implements
/// `futures-rs` AsyncRead/Write
pub struct TcpSocket {
    inner: TcpStream,
}

impl TcpSocket {
    pub fn new(stream: TcpStream) -> Self {
        Self { inner: stream }
    }
}

impl AsyncWrite for TcpSocket {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<Result<usize, Error>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

impl AsyncRead for TcpSocket {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<Result<usize, Error>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl From<TcpStream> for TcpSocket {
    fn from(stream: TcpStream) -> Self {
        Self { inner: stream }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn configure() {
        let mut tcp = TcpTransport::new();
        tcp.set_send_buffer_size(123)
            .set_recv_buffer_size(456)
            .set_nodelay(true)
            .set_ttl(789)
            .set_keepalive(Some(Duration::from_millis(100)));

        assert_eq!(tcp.send_buffer_size, Some(123));
        assert_eq!(tcp.recv_buffer_size, Some(456));
        assert_eq!(tcp.nodelay, Some(true));
        assert_eq!(tcp.ttl, Some(789));
        assert_eq!(tcp.keepalive, Some(Some(Duration::from_millis(100))));
    }
}
