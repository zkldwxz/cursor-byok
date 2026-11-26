//! Carries requests to Cursor's Agents Window over the connection it opened.
use std::{
    collections::HashMap,
    io::Read,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

use bytes::{BufMut, Bytes, BytesMut};
use futures_util::{Stream, StreamExt};
use parking_lot::Mutex;
use prost::Message;
use tokio::sync::{mpsc, watch};
use tokio_stream::wrappers::UnboundedReceiverStream;

use crate::{Error, Result};

const SERVICE: &str = "/agent.v1.AgentHostService/";
const CANCEL: &str = "/cursor.remote-agent-host/CancelRequest";
const CALL_TIMEOUT: Duration = Duration::from_secs(20);
/// Cursor sends nothing near this; a longer length is a broken stream.
const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;
const COMPRESSED: u8 = 1;
const END_STREAM: u8 = 2;

const REQUEST: i32 = 1;
const RESPONSE: i32 = 2;
const ERROR: i32 = 3;

#[derive(Clone, PartialEq, Message)]
pub(super) struct Frame {
    #[prost(string, tag = "1")]
    id: String,
    #[prost(string, tag = "2")]
    method: String,
    #[prost(bytes = "vec", tag = "3")]
    data: Vec<u8>,
    #[prost(int32, tag = "4")]
    kind: i32,
    #[prost(string, tag = "5")]
    error: String,
}

type Answers = mpsc::UnboundedSender<Result<Vec<u8>>>;

/// One connection at a time: Cursor replaces a connection before it drops the old one.
#[derive(Clone)]
pub struct Relay {
    current: Arc<Mutex<Option<Connection>>>,
    next: Arc<AtomicU64>,
    connected: watch::Sender<Option<u64>>,
}

struct Connection {
    id: u64,
    outgoing: mpsc::UnboundedSender<Frame>,
    calls: HashMap<String, Answers>,
}

impl Default for Relay {
    fn default() -> Self {
        Self {
            current: Arc::default(),
            next: Arc::default(),
            connected: watch::channel(None).0,
        }
    }
}

impl Relay {
    /// Names the live connection; changes when Cursor reconnects.
    pub fn connected(&self) -> watch::Receiver<Option<u64>> {
        self.connected.subscribe()
    }

    /// Serves one `ConnectTarget` call: `incoming` is its request body, the
    /// returned stream its response body.
    pub fn accept(
        &self,
        incoming: impl Stream<Item = std::result::Result<Bytes, axum::Error>> + Send + Unpin + 'static,
    ) -> impl Stream<Item = std::result::Result<Bytes, std::convert::Infallible>> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (outgoing, frames) = mpsc::unbounded_channel();
        *self.current.lock() = Some(Connection {
            id,
            outgoing,
            calls: HashMap::new(),
        });
        self.connected.send_replace(Some(id));
        let relay = self.clone();
        tokio::spawn(async move {
            if let Err(error) = relay.listen(id, incoming).await {
                tracing::warn!(%error, "Cursor remote connection ended");
            }
            relay.drop_connection(Some(id));
        });
        // Dropping the connection closes `frames`, which ends the call.
        UnboundedReceiverStream::new(frames)
            .map(|frame| Ok(envelope(0, &frame.encode_to_vec())))
            .chain(futures_util::stream::once(async {
                Ok(envelope(END_STREAM, b"{}"))
            }))
    }

    /// As if Cursor went away.
    #[cfg(test)]
    pub fn close(&self) {
        self.drop_connection(None);
    }

    fn drop_connection(&self, id: Option<u64>) {
        let mut current = self.current.lock();
        if id.is_none() || current.as_ref().map(|connection| connection.id) == id {
            *current = None;
            self.connected.send_replace(None);
        }
    }

    async fn listen(
        &self,
        id: u64,
        mut incoming: impl Stream<Item = std::result::Result<Bytes, axum::Error>> + Unpin,
    ) -> Result<()> {
        let mut buffer = BytesMut::new();
        while let Some(chunk) = incoming.next().await {
            let chunk = chunk
                .map_err(|error| Error::Protocol(format!("Cursor remote connection: {error}")))?;
            buffer.extend_from_slice(&chunk);
            while let Some((flags, payload)) = take_envelope(&mut buffer)? {
                if flags & END_STREAM != 0 {
                    return Ok(());
                }
                self.hear(id, Frame::decode(decompress(flags, payload)?)?);
            }
        }
        Ok(())
    }

    fn hear(&self, id: u64, frame: Frame) {
        let mut current = self.current.lock();
        let Some(connection) = current.as_mut().filter(|connection| connection.id == id) else {
            return;
        };
        match frame.kind {
            // Cursor gives up on a relay that leaves its heartbeats unanswered.
            REQUEST => {
                let _ = connection.outgoing.send(Frame {
                    id: frame.id,
                    method: frame.method,
                    kind: RESPONSE,
                    ..Frame::default()
                });
            }
            kind => {
                let Some(call) = connection.calls.get(&frame.id) else {
                    return;
                };
                let answer = match kind {
                    ERROR => Err(refusal(&frame.error)),
                    _ => Ok(frame.data),
                };
                let failed = answer.is_err();
                if call.send(answer).is_err() || failed {
                    connection.calls.remove(&frame.id);
                }
            }
        }
    }

    pub async fn call<Request: Message, Response: Message + Default>(
        &self,
        method: &str,
        request: &Request,
    ) -> Result<Response> {
        let mut call = self.start(method, request)?;
        let answer = tokio::time::timeout(CALL_TIMEOUT, call.answers.recv())
            .await
            .map_err(|_| Error::Provider(format!("Cursor did not answer {method}")))?
            .ok_or_else(disconnected)??;
        Ok(Response::decode(answer.as_slice())?)
    }

    /// Ends when Cursor finishes it, the connection goes, or it is dropped.
    pub fn stream<Request: Message, Response: Message + Default>(
        &self,
        method: &str,
        request: &Request,
    ) -> Result<impl Stream<Item = Result<Response>>> {
        let mut call = self.start(method, request)?;
        Ok(async_stream::stream! {
            while let Some(answer) = call.answers.recv().await {
                match answer {
                    // Cursor ends a stream with an empty message.
                    Ok(data) if data.is_empty() => return,
                    Ok(data) => yield Response::decode(data.as_slice()).map_err(Error::from),
                    Err(error) => {
                        yield Err(error);
                        return;
                    }
                }
            }
        })
    }

    fn start(&self, method: &str, request: &impl Message) -> Result<Call> {
        let mut current = self.current.lock();
        let connection = current.as_mut().ok_or_else(disconnected)?;
        let id = self.next.fetch_add(1, Ordering::Relaxed).to_string();
        let (sender, answers) = mpsc::unbounded_channel();
        connection
            .outgoing
            .send(Frame {
                id: id.clone(),
                method: format!("{SERVICE}{method}"),
                data: request.encode_to_vec(),
                kind: REQUEST,
                ..Frame::default()
            })
            .map_err(|_| disconnected())?;
        connection.calls.insert(id.clone(), sender);
        Ok(Call {
            relay: self.clone(),
            id,
            answers,
        })
    }
}

struct Call {
    relay: Relay,
    id: String,
    answers: mpsc::UnboundedReceiver<Result<Vec<u8>>>,
}

impl Drop for Call {
    fn drop(&mut self) {
        let mut current = self.relay.current.lock();
        let Some(connection) = current.as_mut() else {
            return;
        };
        if connection.calls.remove(&self.id).is_some() {
            let _ = connection.outgoing.send(Frame {
                id: self.id.clone(),
                method: CANCEL.into(),
                kind: RESPONSE,
                ..Frame::default()
            });
        }
    }
}

fn disconnected() -> Error {
    Error::Provider("Cursor's Agents Window is not connected".into())
}

fn refusal(error: &str) -> Error {
    Error::Provider(format!("Cursor refused the remote request: {error}"))
}

fn envelope(flags: u8, payload: &[u8]) -> Bytes {
    let mut output = BytesMut::with_capacity(5 + payload.len());
    output.put_u8(flags);
    output.put_u32(payload.len() as u32);
    output.extend_from_slice(payload);
    output.freeze()
}

fn take_envelope(buffer: &mut BytesMut) -> Result<Option<(u8, Bytes)>> {
    let Some((&[flags, a, b, c, d], rest)) = buffer.split_first_chunk::<5>() else {
        return Ok(None);
    };
    let length = u32::from_be_bytes([a, b, c, d]) as usize;
    if length > MAX_FRAME_BYTES {
        return Err(Error::Protocol(format!(
            "Cursor remote frame is too long: {length} bytes"
        )));
    }
    if rest.len() < length {
        return Ok(None);
    }
    let _ = buffer.split_to(5);
    Ok(Some((flags, buffer.split_to(length).freeze())))
}

fn decompress(flags: u8, payload: Bytes) -> Result<Bytes> {
    if flags & COMPRESSED == 0 {
        return Ok(payload);
    }
    let mut output = Vec::new();
    flate2::read::GzDecoder::new(payload.as_ref())
        .take(MAX_FRAME_BYTES as u64)
        .read_to_end(&mut output)?;
    Ok(output.into())
}

#[cfg(test)]
pub(super) mod testing {
    use super::*;

    /// Stands in for Cursor at the other end of a relay.
    pub struct FakeCursor {
        sender: mpsc::UnboundedSender<std::result::Result<Bytes, axum::Error>>,
        frames: std::pin::Pin<Box<dyn Stream<Item = Bytes> + Send>>,
    }

    impl FakeCursor {
        pub fn connect(relay: &Relay) -> Self {
            let (sender, body) = mpsc::unbounded_channel();
            let frames = relay
                .accept(UnboundedReceiverStream::new(body))
                .map(|bytes| bytes.unwrap());
            Self {
                sender,
                frames: Box::pin(frames),
            }
        }

        /// The next request: its id, method and message.
        pub async fn next(&mut self) -> Option<(String, String, Vec<u8>)> {
            let mut bytes = BytesMut::from(self.frames.next().await?.as_ref());
            let (flags, payload) = take_envelope(&mut bytes).unwrap().unwrap();
            if flags & END_STREAM != 0 {
                return None;
            }
            let frame = Frame::decode(payload).unwrap();
            Some((frame.id, frame.method, frame.data))
        }

        pub fn answer(&self, id: &str, message: &impl Message) {
            self.send(id, "", message.encode_to_vec(), RESPONSE, "");
        }

        pub fn end(&self, id: &str) {
            self.send(id, "", Vec::new(), RESPONSE, "");
        }

        pub fn refuse(&self, id: &str, error: &str) {
            self.send(id, "", Vec::new(), ERROR, error);
        }

        pub fn ask(&self, id: &str, method: &str) {
            self.send(id, method, Vec::new(), REQUEST, "");
        }

        fn send(&self, id: &str, method: &str, data: Vec<u8>, kind: i32, error: &str) {
            let frame = Frame {
                id: id.into(),
                method: method.into(),
                data,
                kind,
                error: error.into(),
            };
            self.send_raw(envelope(0, &frame.encode_to_vec()));
        }

        pub fn send_raw(&self, bytes: Bytes) {
            let _ = self.sender.send(Ok(bytes));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::{testing::FakeCursor, *};
    use crate::cursor::protocol::proto::agent::v1 as pb;

    fn session(id: &str) -> pb::ListAgentHostSessionsResponse {
        pb::ListAgentHostSessionsResponse {
            sessions: vec![pb::AgentHostSession {
                session_id: id.into(),
                ..Default::default()
            }],
        }
    }

    async fn list(relay: Relay) -> Result<pb::ListAgentHostSessionsResponse> {
        relay
            .call("ListSessions", &pb::ListAgentHostSessionsRequest::default())
            .await
    }

    #[tokio::test]
    async fn answers_reach_the_call_they_belong_to() {
        let relay = Relay::default();
        let mut cursor = FakeCursor::connect(&relay);
        let first = tokio::spawn(list(relay.clone()));
        let (one, _, _) = cursor.next().await.unwrap();
        let second = tokio::spawn(list(relay.clone()));
        let (two, _, _) = cursor.next().await.unwrap();
        cursor.answer(&two, &session("second"));
        cursor.answer(&one, &session("first"));
        assert_eq!(first.await.unwrap().unwrap(), session("first"));
        assert_eq!(second.await.unwrap().unwrap(), session("second"));
    }

    #[tokio::test]
    async fn a_refused_call_says_why() {
        let relay = Relay::default();
        let mut cursor = FakeCursor::connect(&relay);
        let call = tokio::spawn(list(relay));
        let (id, _, _) = cursor.next().await.unwrap();
        cursor.refuse(&id, r#"{"code":5}"#);
        let error = call.await.unwrap().unwrap_err().to_string();
        assert!(error.contains(r#"{"code":5}"#), "{error}");
    }

    #[tokio::test]
    async fn a_lost_connection_fails_the_calls_under_way() {
        let relay = Relay::default();
        let mut cursor = FakeCursor::connect(&relay);
        let call = tokio::spawn(list(relay.clone()));
        cursor.next().await.unwrap();
        drop(cursor);
        assert!(call.await.unwrap().is_err());
        assert_eq!(*relay.connected().borrow(), None);
        assert!(list(relay).await.is_err());
    }

    #[tokio::test]
    async fn a_replaced_connection_does_not_take_the_new_one_down() {
        let relay = Relay::default();
        let old = FakeCursor::connect(&relay);
        let mut new = FakeCursor::connect(&relay);
        drop(old);
        tokio::task::yield_now().await;
        let call = tokio::spawn(list(relay.clone()));
        let (id, _, _) = new.next().await.unwrap();
        new.answer(&id, &session("kept"));
        assert_eq!(call.await.unwrap().unwrap(), session("kept"));
    }

    #[tokio::test]
    async fn heartbeats_are_answered() {
        let relay = Relay::default();
        let mut cursor = FakeCursor::connect(&relay);
        cursor.ask("beat", "/cursor.remote-agent-host/Heartbeat");
        let (id, method, _) = cursor.next().await.unwrap();
        assert_eq!(
            (id.as_str(), method.as_str()),
            ("beat", "/cursor.remote-agent-host/Heartbeat")
        );
    }

    #[tokio::test]
    async fn a_stream_ends_on_an_empty_message_and_a_dropped_one_is_cancelled() {
        let relay = Relay::default();
        let mut cursor = FakeCursor::connect(&relay);
        let attach = pb::AttachAgentHostSessionRequest::default();
        let event = pb::AgentHostSessionEvent {
            event_id: 7,
            event: None,
        };

        let mut finished = Box::pin(
            relay
                .stream::<_, pb::AgentHostSessionEvent>("AttachSession", &attach)
                .unwrap(),
        );
        let (id, _, _) = cursor.next().await.unwrap();
        cursor.answer(&id, &event);
        cursor.end(&id);
        assert_eq!(finished.next().await.unwrap().unwrap(), event);
        assert!(finished.next().await.is_none());
        drop(finished);
        let (_, method, _) = cursor.next().await.unwrap();
        assert_eq!(method, CANCEL);

        let dropped = relay
            .stream::<_, pb::AgentHostSessionEvent>("AttachSession", &attach)
            .unwrap();
        let (id, _, _) = cursor.next().await.unwrap();
        drop(dropped);
        let (cancelled, method, _) = cursor.next().await.unwrap();
        assert_eq!((cancelled, method.as_str()), (id, CANCEL));
    }

    #[tokio::test]
    async fn compressed_and_split_frames_are_read_whole() {
        let relay = Relay::default();
        let mut cursor = FakeCursor::connect(&relay);
        let call = tokio::spawn(list(relay));
        let (id, _, _) = cursor.next().await.unwrap();
        let frame = Frame {
            id,
            data: session("zipped").encode_to_vec(),
            kind: RESPONSE,
            ..Frame::default()
        };
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(&frame.encode_to_vec()).unwrap();
        let bytes = envelope(COMPRESSED, &encoder.finish().unwrap());
        cursor.send_raw(bytes.slice(..7));
        cursor.send_raw(bytes.slice(7..));
        assert_eq!(call.await.unwrap().unwrap(), session("zipped"));
    }
}
