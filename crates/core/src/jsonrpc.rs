//! Transport-generic JSON-RPC 2.0 client for sbt's server.
//!
//! One actor task owns the socket, the outbound queue and the map of pending
//! requests, so ordering is structural: it registers a reply slot before it
//! writes the request, forwards every notification before any later response
//! resolves, and when the socket closes it drops the map, which fails every
//! pending request with [`RequestError::ConnectionClosed`].

pub mod framing;
pub mod message;

use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpStream, UnixStream};
use tokio::sync::{mpsc, oneshot};

use self::framing::{FrameDecoder, encode_frame};
pub use self::message::{Incoming, Notification, RequestId, RpcError};
use self::message::{encode_method_not_found, encode_request, parse_incoming};

/// A bidirectional socket of either transport sbt may publish in
/// `active.json`.
pub trait Io: AsyncRead + AsyncWrite + Send + Unpin {}

impl<T: AsyncRead + AsyncWrite + Send + Unpin> Io for T {}

pub type BoxedIo = Box<dyn Io>;

const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;
const OUTBOUND_QUEUE: usize = 64;

/// # Errors
///
/// The socket could not be opened.
pub async fn connect_tcp(addr: SocketAddr) -> io::Result<BoxedIo> {
    let stream = TcpStream::connect(addr).await?;
    stream.set_nodelay(true)?;
    Ok(Box::new(stream))
}

/// # Errors
///
/// The socket could not be opened.
pub async fn connect_unix(path: &Path) -> io::Result<BoxedIo> {
    Ok(Box::new(UnixStream::connect(path).await?))
}

#[derive(Debug, Error)]
pub enum RequestError {
    #[error("sbt replied with {0}")]
    Rpc(RpcError),
    #[error("connection to sbt closed")]
    ConnectionClosed,
}

/// Cloneable handle to one connection's actor.
#[derive(Debug, Clone)]
pub struct Connection {
    outbound: mpsc::Sender<Outbound>,
    next_id: Arc<AtomicU64>,
}

#[derive(Debug)]
enum Outbound {
    Request {
        id: RequestId,
        method: String,
        params: Value,
        reply: oneshot::Sender<Result<Value, RpcError>>,
    },
}

impl Connection {
    /// Start the actor over an open socket. Every notification the server
    /// sends is delivered, in order, to `notifications`; a full channel
    /// applies backpressure to the socket rather than dropping anything.
    #[must_use]
    pub fn open(io: BoxedIo, notifications: mpsc::Sender<Notification>) -> Self {
        let (outbound, inbox) = mpsc::channel(OUTBOUND_QUEUE);
        tokio::spawn(Actor::new(io, inbox, notifications).run());
        Self {
            outbound,
            next_id: Arc::new(AtomicU64::new(1)),
        }
    }

    /// Send a request and wait for its response.
    ///
    /// # Errors
    ///
    /// `Rpc` when the server answered with an error member; `ConnectionClosed`
    /// when the socket closed before it answered.
    pub async fn request(&self, method: &str, params: Value) -> Result<Value, RequestError> {
        let id = RequestId::new(self.next_id.fetch_add(1, Ordering::Relaxed));
        let (reply, response) = oneshot::channel();
        self.outbound
            .send(Outbound::Request {
                id,
                method: method.to_owned(),
                params,
                reply,
            })
            .await
            .map_err(|_| RequestError::ConnectionClosed)?;
        response
            .await
            .map_err(|_| RequestError::ConnectionClosed)?
            .map_err(RequestError::Rpc)
    }

    /// True once the actor has exited; requests will fail with
    /// `ConnectionClosed`.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.outbound.is_closed()
    }
}

struct Actor {
    io: BoxedIo,
    inbox: mpsc::Receiver<Outbound>,
    notifications: mpsc::Sender<Notification>,
    pending: HashMap<RequestId, oneshot::Sender<Result<Value, RpcError>>>,
    decoder: FrameDecoder,
}

enum Step {
    Continue,
    Stop,
}

impl Actor {
    fn new(
        io: BoxedIo,
        inbox: mpsc::Receiver<Outbound>,
        notifications: mpsc::Sender<Notification>,
    ) -> Self {
        Self {
            io,
            inbox,
            notifications,
            pending: HashMap::new(),
            decoder: FrameDecoder::new(MAX_BODY_BYTES),
        }
    }

    async fn run(mut self) {
        let mut chunk = [0u8; 8192];
        loop {
            let step = tokio::select! {
                outbound = self.inbox.recv() => match outbound {
                    Some(outbound) => self.send(outbound).await,
                    None => Step::Stop,
                },
                read = self.io.read(&mut chunk) => match read {
                    Ok(0) => Step::Stop,
                    Ok(n) => self.receive(&chunk[..n]).await,
                    Err(error) => {
                        tracing::debug!(%error, "sbt connection read failed");
                        Step::Stop
                    }
                },
            };
            if matches!(step, Step::Stop) {
                break;
            }
        }
        tracing::debug!(pending = self.pending.len(), "sbt connection closed");
    }

    async fn send(&mut self, outbound: Outbound) -> Step {
        let Outbound::Request {
            id,
            method,
            params,
            reply,
        } = outbound;
        let body = encode_request(id, &method, &params);
        tracing::trace!(frame = %String::from_utf8_lossy(&body), "-> sbt");
        self.pending.insert(id, reply);
        self.write(&body).await
    }

    async fn write(&mut self, body: &[u8]) -> Step {
        match self.io.write_all(&encode_frame(body)).await {
            Ok(()) => Step::Continue,
            Err(error) => {
                tracing::debug!(%error, "sbt connection write failed");
                Step::Stop
            }
        }
    }

    async fn receive(&mut self, bytes: &[u8]) -> Step {
        self.decoder.push(bytes);
        loop {
            match self.decoder.next_frame() {
                Ok(Some(body)) => {
                    tracing::trace!(frame = %String::from_utf8_lossy(&body), "<- sbt");
                    if matches!(self.dispatch(&body).await, Step::Stop) {
                        return Step::Stop;
                    }
                }
                Ok(None) => return Step::Continue,
                Err(error) => {
                    tracing::error!(%error, "sbt sent an unframeable message");
                    return Step::Stop;
                }
            }
        }
    }

    async fn dispatch(&mut self, body: &[u8]) -> Step {
        match parse_incoming(body) {
            Ok(Incoming::Response { id, outcome }) => {
                if let Some(reply) = self.pending.remove(&id) {
                    if reply.send(outcome).is_err() {
                        tracing::trace!(%id, "requester gave up before the response");
                    }
                } else {
                    tracing::warn!(%id, "response to an unknown request");
                }
                Step::Continue
            }
            Ok(Incoming::Notification(notification)) => {
                if self.notifications.send(notification).await.is_err() {
                    tracing::trace!("notification consumer is gone");
                }
                Step::Continue
            }
            Ok(Incoming::ServerRequest { id, method, .. }) => {
                tracing::debug!(%method, "declining a server-initiated request");
                self.write(&encode_method_not_found(&id, &method)).await
            }
            Err(error) => {
                tracing::warn!(%error, "ignoring an unparseable message from sbt");
                Step::Continue
            }
        }
    }
}
