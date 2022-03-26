use crate::prelude::IPCResult;
use crate::protocol::{AsyncProtocolStream, AsyncProtocolStreamSplit, AsyncStreamProtocolListener};
use async_trait::async_trait;
use std::net::SocketAddr;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};

#[async_trait]
impl AsyncStreamProtocolListener for TcpListener {
    type AddressType = SocketAddr;
    type RemoteAddressType = SocketAddr;
    type Stream = TcpStream;
    type ListenerOptions = ();

    async fn protocol_bind(
        address: Self::AddressType,
        _: Self::ListenerOptions,
    ) -> IPCResult<Self> {
        let listener = TcpListener::bind(address).await?;

        Ok(listener)
    }

    async fn protocol_accept(&self) -> IPCResult<(Self::Stream, Self::RemoteAddressType)> {
        let connection = self.accept().await?;

        Ok(connection)
    }
}

impl AsyncProtocolStreamSplit for TcpStream {
    type OwnedSplitReadHalf = OwnedReadHalf;
    type OwnedSplitWriteHalf = OwnedWriteHalf;

    fn protocol_into_split(self) -> (Self::OwnedSplitReadHalf, Self::OwnedSplitWriteHalf) {
        self.into_split()
    }
}

#[async_trait]
impl AsyncProtocolStream for TcpStream {
    type AddressType = SocketAddr;
    type StreamOptions = ();

    async fn protocol_connect(
        address: Self::AddressType,
        _: Self::StreamOptions,
    ) -> IPCResult<Self> {
        let stream = TcpStream::connect(address).await?;

        Ok(stream)
    }
}
