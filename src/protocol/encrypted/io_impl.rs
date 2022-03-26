use crate::prelude::encrypted::{
    EncryptedPackage, EncryptedReadStream, EncryptedStream, EncryptedWriteStream,
};
use crate::prelude::AsyncProtocolStream;
use crate::protocol::encrypted::crypt_handling::CipherBox;
use bytes::{Buf, BufMut, Bytes};
use std::cmp::min;
use std::io;
use std::io::Error;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadBuf};

const WRITE_BUF_SIZE: usize = 1024;

impl<T: AsyncProtocolStream> Unpin for EncryptedStream<T> {}

impl<T: AsyncProtocolStream> AsyncWrite for EncryptedStream<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, Error>> {
        Pin::new(&mut self.write_half).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        Pin::new(&mut self.write_half).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        Pin::new(&mut self.write_half).poll_shutdown(cx)
    }
}

impl<T: AsyncProtocolStream> AsyncRead for EncryptedStream<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.read_half).poll_read(cx, buf)
    }
}

impl<T: 'static + AsyncRead + Unpin + Send + Sync> Unpin for EncryptedReadStream<T> {}

impl<T: 'static + AsyncRead + Send + Sync + Unpin> AsyncRead for EncryptedReadStream<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if self.fut.is_none() {
            let max_copy = min(buf.remaining(), self.remaining.len());
            let bytes = self.remaining.copy_to_bytes(max_copy);
            buf.put_slice(&bytes);

            if buf.remaining() > 0 {
                let mut reader = self.inner.take().unwrap();
                let cipher = self.cipher.take().unwrap();

                self.fut = Some(Box::pin(async move {
                    let package = match EncryptedPackage::from_async_read(&mut reader).await {
                        Ok(p) => p,
                        Err(e) => {
                            return (Err(e), reader, cipher);
                        }
                    };
                    match cipher.decrypt(package.into_inner()) {
                        Ok(bytes) => (Ok(bytes), reader, cipher),
                        Err(e) => (Err(e), reader, cipher),
                    }
                }));
            }
        }
        if self.fut.is_some() {
            match self.fut.as_mut().unwrap().as_mut().poll(cx) {
                Poll::Ready((result, reader, cipher)) => {
                    self.inner = Some(reader);
                    self.cipher = Some(cipher);
                    match result {
                        Ok(bytes) => {
                            self.remaining.put(bytes);
                            let max_copy = min(self.remaining.len(), buf.remaining());
                            let bytes = self.remaining.copy_to_bytes(max_copy);
                            self.fut = None;
                            buf.put_slice(&bytes);

                            if buf.remaining() == 0 {
                                Poll::Ready(Ok(()))
                            } else {
                                Poll::Pending
                            }
                        }
                        Err(e) => Poll::Ready(Err(e)),
                    }
                }
                Poll::Pending => Poll::Pending,
            }
        } else {
            Poll::Ready(Ok(()))
        }
    }
}

impl<T: 'static + AsyncWrite + Unpin + Send + Sync> Unpin for EncryptedWriteStream<T> {}

impl<T: 'static + AsyncWrite + Unpin + Send + Sync> AsyncWrite for EncryptedWriteStream<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, Error>> {
        if buf.remaining() > 0 {
            let buf = unsafe { std::mem::transmute::<_, &'static [u8]>(buf) };
            self.buffer.put(Bytes::from(buf));

            if self.fut_write.is_none() && self.buffer.len() >= WRITE_BUF_SIZE {
                let buffer_len = self.buffer.len();
                let max_copy = min(u32::MAX as usize, buffer_len);
                let plaintext = self.buffer.copy_to_bytes(max_copy);
                let writer = self.inner.take().unwrap();
                let cipher = self.cipher.take().unwrap();

                self.fut_write = Some(Box::pin(write_bytes(plaintext, writer, cipher)))
            }
        }
        if self.fut_write.is_some() {
            match self.fut_write.as_mut().unwrap().as_mut().poll(cx) {
                Poll::Ready((result, writer, cipher)) => {
                    self.inner = Some(writer);
                    self.cipher = Some(cipher);
                    self.fut_write = None;

                    Poll::Ready(result.map(|_| buf.len()))
                }
                Poll::Pending => Poll::Pending,
            }
        } else {
            Poll::Ready(Ok(buf.len()))
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        let buffer_len = self.buffer.len();

        if !self.buffer.is_empty() && self.fut_flush.is_none() {
            let max_copy = min(u32::MAX as usize, buffer_len);
            let plaintext = self.buffer.copy_to_bytes(max_copy);
            let writer = self.inner.take().unwrap();
            let cipher = self.cipher.take().unwrap();

            self.fut_flush = Some(Box::pin(async move {
                let (result, mut writer, cipher) = write_bytes(plaintext, writer, cipher).await;
                if result.is_err() {
                    return (result, writer, cipher);
                }
                if let Err(e) = writer.flush().await {
                    (Err(e), writer, cipher)
                } else {
                    (Ok(()), writer, cipher)
                }
            }))
        }
        match self.fut_flush.as_mut().unwrap().as_mut().poll(cx) {
            Poll::Ready((result, writer, cipher)) => {
                self.inner = Some(writer);
                self.cipher = Some(cipher);
                self.fut_flush = None;

                Poll::Ready(result)
            }
            Poll::Pending => Poll::Pending,
        }
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        if self.fut_shutdown.is_none() {
            match self.as_mut().poll_flush(cx) {
                Poll::Ready(result) => match result {
                    Ok(_) => {
                        let mut writer = self.inner.take().unwrap();
                        self.fut_shutdown = Some(Box::pin(async move { writer.shutdown().await }));
                        Poll::Pending
                    }
                    Err(e) => Poll::Ready(Err(e)),
                },
                Poll::Pending => Poll::Pending,
            }
        } else {
            match self.fut_shutdown.as_mut().unwrap().as_mut().poll(cx) {
                Poll::Ready(result) => {
                    self.fut_shutdown = None;
                    Poll::Ready(result)
                }
                Poll::Pending => Poll::Pending,
            }
        }
    }
}

async fn write_bytes<T: AsyncWrite + Unpin>(
    bytes: Bytes,
    mut writer: T,
    cipher: CipherBox,
) -> (io::Result<()>, T, CipherBox) {
    let encrypted_bytes = match cipher.encrypt(bytes) {
        Ok(b) => b,
        Err(e) => {
            return (Err(e), writer, cipher);
        }
    };
    let package_bytes = EncryptedPackage::new(encrypted_bytes).into_bytes();
    if let Err(e) = writer.write_all(&package_bytes[..]).await {
        return (Err(e), writer, cipher);
    }

    (Ok(()), writer, cipher)
}
