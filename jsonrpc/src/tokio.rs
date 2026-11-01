use std::{io, marker::PhantomData, pin::pin, task::Poll};

use futures_util::{Sink, Stream, StreamExt, ready, stream::Forward};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::codec::{Decoder, Encoder, FramedRead, FramedWrite};

use crate::server::{JsonRpcResponse, Server, ServerBuilder};

pub struct TokioServer<C, R, W>(
    Forward<
        TryStreamAdapter<<C as Encoder<String>>::Error, Server<CodecCompatRead<R, C>>>,
        CodecCompatWrite<W, C>,
    >,
)
where
    C: Decoder<Item = String> + Encoder<String> + Clone,
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin;

impl<C, R, W> Future for TokioServer<C, R, W>
where
    C: Decoder<Item = String> + Encoder<String> + Clone,
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    type Output = Result<(), <C as Encoder<String>>::Error>;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Self::Output> {
        let fut = pin!(&mut self.0);
        fut.poll(cx)
    }
}

pub trait BuilderTokioServerExt {
    fn build_with_codec<Codec, Read, Write>(
        self,
        codec: Codec,
        read: Read,
        write: Write,
    ) -> TokioServer<Codec, Read, Write>
    where
        Codec: Encoder<String> + Decoder<Item = String> + Clone,
        Read: AsyncRead + Unpin,
        Write: AsyncWrite + Unpin;
}

impl BuilderTokioServerExt for ServerBuilder {
    fn build_with_codec<Codec, Read, Write>(
        self,
        codec: Codec,
        read: Read,
        write: Write,
    ) -> TokioServer<Codec, Read, Write>
    where
        Codec: Encoder<String> + Decoder<Item = String> + Clone,
        Read: AsyncRead + Unpin,
        Write: AsyncWrite + Unpin,
    {
        let reader = FramedRead::new(read, codec.clone());
        let writer = FramedWrite::new(write, codec);

        let server = TryStreamAdapter(self.build(CodecCompatRead(reader)), PhantomData)
            .forward(CodecCompatWrite(writer));

        TokioServer(server)
    }
}

struct CodecCompatRead<S, D>(FramedRead<S, D>);

impl<S: AsyncRead + Unpin, D: Decoder<Item = String>> Stream for CodecCompatRead<S, D> {
    type Item = String;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let fut = pin!(&mut self.0);
        let Some(item) = ready!(fut.poll_next(cx)) else {
            return Poll::Ready(None);
        };

        // ToDo: Log error here (?)
        Poll::Ready(item.ok())
    }
}

struct CodecCompatWrite<S, E>(FramedWrite<S, E>);

impl<T, E> Sink<JsonRpcResponse> for CodecCompatWrite<T, E>
where
    T: AsyncWrite + Unpin,
    E: Encoder<String>,
    E::Error: From<io::Error>,
{
    type Error = E::Error;

    fn poll_ready(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        let fut = pin!(&mut self.0);
        fut.poll_ready(cx)
    }

    fn start_send(
        mut self: std::pin::Pin<&mut Self>,
        item: JsonRpcResponse,
    ) -> Result<(), Self::Error> {
        let result = serde_json::to_string(&item).unwrap();
        let pin = pin!(&mut self.0);
        pin.start_send(result)
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        let fut = pin!(&mut self.0);
        fut.poll_flush(cx)
    }

    fn poll_close(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        let fut = pin!(&mut self.0);
        fut.poll_close(cx)
    }
}

struct TryStreamAdapter<E, S>(S, PhantomData<E>);

impl<E, S: Stream + Unpin> Stream for TryStreamAdapter<E, S> {
    type Item = Result<S::Item, E>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        let fut = pin!(&mut self.0);
        let res = ready!(fut.poll_next(cx));
        Poll::Ready(res.map(Ok))
    }
}

impl<E, S> Unpin for TryStreamAdapter<E, S> {}
