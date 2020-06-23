//! Implementation of the language server protocol.

use std::fmt::{self, Display};

use atoi::atoi;
use bytes::{Buf, BufMut, BytesMut};
use httparse::{Status, EMPTY_HEADER};
use log::*;
use serde::de::{self, Unexpected, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use thiserror::Error;
use tokio::io;
use tokio_util::codec::{Decoder, Encoder};

const MAX_HEADERS: usize = 16;

#[derive(Debug, PartialEq, Eq)]
pub enum Message {
    Request(Request),
    Response(Response),
    Notification(Notification),
}

impl Message {
    pub fn request<R>(id: Id, params: R::Params) -> Message
    where
        R: lsp_types::request::Request,
        <R as lsp_types::request::Request>::Params: Serialize,
    {
        Message::Request(Request {
            id,
            method: String::from(R::METHOD),
            params: Some(serde_json::to_value(params).expect("could not serialize request")),
        })
    }

    pub fn notification<N>(params: N::Params) -> Message
    where
        N: lsp_types::notification::Notification,
        <N as lsp_types::notification::Notification>::Params: Serialize,
    {
        Message::Notification(Notification {
            method: String::from(N::METHOD),
            params: Some(serde_json::to_value(params).expect("could not serialize notification")),
        })
    }
}

impl<'de> Deserialize<'de> for Message {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Debug, Deserialize)]
        #[serde(deny_unknown_fields)]
        pub struct RawMessage {
            jsonrpc: String,
            #[serde(default, deserialize_with = "double_option")]
            id: Option<Option<Id>>,
            method: Option<String>,
            #[serde(default, deserialize_with = "double_option")]
            params: Option<Value>,
            #[serde(default, deserialize_with = "double_option")]
            result: Option<Value>,
            #[serde(default, deserialize_with = "double_option")]
            error: Option<ResponseError>,
        }

        fn double_option<'de, T, D>(de: D) -> Result<Option<T>, D::Error>
        where
            T: Deserialize<'de>,
            D: Deserializer<'de>,
        {
            Deserialize::deserialize(de).map(Some)
        }

        let val = RawMessage::deserialize(deserializer)?;

        if val.jsonrpc != "2.0" {
            return Err(de::Error::invalid_value(
                Unexpected::Other("JSON-RPC protocol version"),
                &"2.0",
            ));
        }

        assert_eq!(val.jsonrpc, "2.0");

        let message = if val.result.is_some() || val.error.is_some() {
            let id = match val.id {
                Some(Some(id)) => Some(id),
                Some(None) => None,
                None => return Err(de::Error::missing_field("id")),
            };

            Message::Response(Response {
                id,
                result: match (val.result, val.error) {
                    (Some(res), None) => Ok(res),
                    (None, Some(err)) => Err(err),
                    _ => return Err(de::Error::custom("expected exactly one of result or error")),
                },
            })
        } else {
            let params = val.params;

            let id = match val.id {
                Some(Some(id)) => Some(id),
                Some(None) => {
                    return Err(de::Error::invalid_value(
                        Unexpected::Other("null"),
                        &"string or integer",
                    ))
                }
                None => None,
            };

            let method = val
                .method
                .ok_or_else(|| de::Error::missing_field("method"))?;

            if let Some(id) = id {
                Message::Request(Request { id, method, params })
            } else {
                Message::Notification(Notification { method, params })
            }
        };

        Ok(message)
    }
}

impl Serialize for Message {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let fields = match self {
            Message::Request(req) => 2 + if req.params.is_some() { 1 } else { 0 },
            Message::Response(_) => 2,
            Message::Notification(not) => 1 + if not.params.is_some() { 1 } else { 0 },
        };

        let mut map = serializer.serialize_map(Some(1 + fields))?;
        map.serialize_entry("jsonrpc", "2.0")?;

        match self {
            Message::Request(request) => {
                map.serialize_entry("id", &request.id)?;
                map.serialize_entry("method", &request.method)?;

                if let Some(params) = &request.params {
                    map.serialize_entry("params", params)?;
                }
            }
            Message::Response(response) => {
                map.serialize_entry("id", &response.id)?;

                match &response.result {
                    Ok(result) => map.serialize_entry("result", result)?,
                    Err(error) => map.serialize_entry("error", error)?,
                }
            }
            Message::Notification(notification) => {
                map.serialize_entry("method", &notification.method)?;

                if let Some(params) = &notification.params {
                    map.serialize_entry("params", &params)?;
                }
            }
        }

        map.end()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct Id(String);

impl From<u64> for Id {
    fn from(id: u64) -> Self {
        Id(id.to_string())
    }
}

impl Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Id {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct IdVisitor;

        impl<'de> Visitor<'de> for IdVisitor {
            type Value = Id;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("request ID as number or string")
            }

            fn visit_u64<E>(self, id: u64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(Id(id.to_string()))
            }

            fn visit_str<E>(self, id: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(Id(String::from(id)))
            }
        }

        deserializer.deserialize_any(IdVisitor)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Request {
    pub id: Id,
    pub method: String,
    pub params: Option<Value>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Response {
    pub id: Option<Id>,
    pub result: Result<Value, ResponseError>,
}

impl Response {
    pub fn method_not_found(id: Id) -> Self {
        Response {
            id: Some(id),
            result: Err(ResponseError {
                code: ResponseError::METHOD_NOT_FOUND,
                message: String::from("method not found"),
                data: None,
            }),
        }
    }
}

#[derive(Debug, PartialEq, Eq, Deserialize, Serialize, Error)]
pub struct ResponseError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl ResponseError {
    const METHOD_NOT_FOUND: i64 = -32601;
}

impl Display for ResponseError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let payload = self
            .data
            .as_ref()
            .map(|data| {
                serde_json::to_string_pretty(data)
                    .unwrap_or_else(|e| format!("<unable to deserialize: {}>", e))
            })
            .unwrap_or_else(|| String::from("none"));

        write!(f, "{}: {}\npayload: {}", self.code, self.message, payload)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Notification {
    pub method: String,
    pub params: Option<Value>,
}

#[derive(Debug, Error)]
pub enum LspError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("error parsing HTTP headers: {0}")]
    Headers(#[from] httparse::Error),
    #[error("no Content-Length header")]
    MissingContentLength,
    #[error("Content-Length header was not a number")]
    InvalidContentLength,
    #[error("error parsing json: {0}")]
    Json(#[from] serde_json::Error),
}

pub struct LspCodec;

impl Encoder<Message> for LspCodec {
    type Error = io::Error;

    fn encode(&mut self, item: Message, dst: &mut BytesMut) -> Result<(), Self::Error> {
        let message = serde_json::to_vec(&item).expect("message encoding should never fail");

        trace!("-> {}", String::from_utf8_lossy(&message));

        dst.put(format!("Content-Length: {}\r\n", message.len()).as_bytes());
        dst.put(&b"Content-Type: application/vscode-jsonrpc; charset=utf-8\r\n"[..]);
        dst.put(&b"\r\n"[..]);
        dst.put(message.as_slice());

        Ok(())
    }
}

impl Decoder for LspCodec {
    type Item = Message;
    type Error = LspError;

    fn decode(&mut self, buf: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let mut headers = [EMPTY_HEADER; MAX_HEADERS];

        let (bytes_read, content_length) = match httparse::parse_headers(&buf, &mut headers)? {
            Status::Partial => return Ok(None),
            Status::Complete((bytes_read, headers)) => {
                let content_length: usize = headers
                    .iter()
                    .find(|header| header.name == "Content-Length")
                    .ok_or(LspError::MissingContentLength)
                    .and_then(|header| atoi(header.value).ok_or(LspError::InvalidContentLength))?;
                (bytes_read, content_length)
            }
        };

        if bytes_read + content_length > buf.len() {
            return Ok(None);
        }

        buf.advance(bytes_read);
        let content = buf.split_to(content_length).freeze();

        trace!("<- {}", String::from_utf8_lossy(&content));

        let message = serde_json::from_slice(&content)?;
        Ok(Some(message))
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::io::Cursor;

    use assert_matches::assert_matches;
    use futures::TryStreamExt;
    use lsp_types::{
        lsp_notification, lsp_request, InitializeResult, MessageType, ShowMessageParams,
        ShowMessageRequestParams,
    };
    use serde::Deserialize;
    use serde_json::{json, Map, Value};
    use tokio_util::codec::FramedRead;

    use super::{Id, LspCodec, LspError, Message, Notification, Response};

    #[test]
    fn serialize_request() -> Result<(), Box<dyn Error>> {
        let request = Message::request::<lsp_request!("window/showMessageRequest")>(
            Id::from(0),
            ShowMessageRequestParams {
                typ: MessageType::Error,
                message: String::from("error message"),
                actions: None,
            },
        );

        assert_eq!(
            serde_json::to_value(&request)?,
            json!({
                "jsonrpc": "2.0",
                "id": "0",
                "method": "window/showMessageRequest",
                "params": {
                    "message": "error message",
                    "type": 1
                }
            })
        );

        Ok(())
    }

    #[test]
    fn serialize_notification() -> Result<(), Box<dyn Error>> {
        let notification =
            Message::notification::<lsp_notification!("window/showMessage")>(ShowMessageParams {
                typ: MessageType::Warning,
                message: String::from("Hello, world!"),
            });

        assert_eq!(
            serde_json::to_value(&notification)?,
            json!({
                "jsonrpc": "2.0",
                "method": "window/showMessage",
                "params": {
                    "message": "Hello, world!",
                    "type": 2
                }
            }),
        );

        Ok(())
    }

    #[test]
    fn serialize_response_result() -> Result<(), Box<dyn Error>> {
        let response = Message::Response(Response {
            id: Some(Id::from(1)),
            result: Ok(serde_json::to_value(InitializeResult::default())?),
        });

        assert_eq!(
            serde_json::to_value(&response)?,
            json!({
                "jsonrpc": "2.0",
                "id": "1",
                "result": {
                    "capabilities": {}
                }
            }),
        );

        Ok(())
    }

    #[test]
    fn deserialize_request_string_id() {
        let json = json!({ "jsonrpc": "2.0", "id": "1", "method": "foo" });

        let request = assert_matches!(Message::deserialize(json), Ok(Message::Request(req)) => req);

        assert_eq!(request.id, 1.into());
    }

    #[test]
    fn deserialize_request_reject_missing_method() {
        let json = json!({ "jsonrpc": "2.0", "id": 1});

        let err = Message::deserialize(json).unwrap_err();

        assert!(err.to_string().contains("missing field `method`"));
    }

    #[test]
    fn deserialize_response() {
        let json = json!({ "jsonrpc": "2.0", "id": 1, "result": null});

        let response =
            assert_matches!(Message::deserialize(json), Ok(Message::Response(res)) => res);

        assert_eq!(response.result, Ok(Value::Null));
    }

    #[test]
    fn deserialize_reject_unknown_fields() {
        let json = json!({ "jsonrpc": "2.0", "id": 1, "result": null, "extra": 1 });

        let err = Message::deserialize(json).unwrap_err();

        assert!(err.to_string().contains("unknown"));
    }

    #[test]
    fn deserialize_reject_old_json_rpc_version() {
        let json = json!({ "jsonrpc": "1.0" });

        let err = Message::deserialize(json).unwrap_err();

        assert!(err.to_string().contains("protocol version"));
    }

    #[test]
    fn deserialize_response_reject_both_result_and_error() {
        let json = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": null,
            "error": {
                "code": 1,
                "message": "error"
            }
        });

        let err = Message::deserialize(json).unwrap_err();

        assert!(err
            .to_string()
            .contains("expected exactly one of result or error"));
    }

    #[test]
    fn deserialize_response_reject_missing_id() {
        let json = json!({ "jsonrpc": "2.0", "result": null });

        let err = Message::deserialize(json).unwrap_err();
        assert_eq!(err.to_string(), "missing field `id`");
    }

    #[test]
    fn deserialize_response_with_null_id() -> Result<(), Box<dyn Error>> {
        let json = json!({
        "jsonrpc": "2.0",
        "id": null,
        "error": {
            "code": 1,
            "message": "error",
        }});

        let response = assert_matches!(Message::deserialize(json)?, Message::Response(res) => res);
        assert_eq!(response.id, None);

        Ok(())
    }

    #[test]
    fn deserialize_notification_missing_method() {
        let json = json!({ "jsonrpc": "2.0" });

        let err = Message::deserialize(json).unwrap_err();

        assert!(err.to_string().contains("missing field `method`"));
    }

    #[tokio::test]
    async fn decode_frame() {
        let frame = concat!(
            "Content-Length: 52\r\n\r\n",
            r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#,
        );
        let messages: Vec<Message> = FramedRead::new(frame.as_bytes(), LspCodec)
            .try_collect()
            .await
            .unwrap();
        assert_eq!(
            messages,
            vec![Message::Notification(Notification {
                method: String::from("initialized"),
                params: Some(Value::Object(Map::new())),
            })]
        );
    }

    #[tokio::test]
    async fn decode_multiple_frames() {
        let frames = concat!(
            "Content-Length: 52\r\n\r\n",
            r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#,
            "Content-Length: 44\r\n\r\n",
            r#"{"jsonrpc":"2.0","id":1,"method":"shutdown"}"#,
        );

        let messages: Vec<Message> = FramedRead::new(frames.as_bytes(), LspCodec)
            .try_collect()
            .await
            .unwrap();
        assert_matches!(
            messages.as_slice(),
            [Message::Notification(_), Message::Request(_)]
        );
    }

    #[tokio::test]
    async fn decode_eof() {
        let frame = Cursor::new(b"");
        let codec: Vec<Message> = FramedRead::new(frame, LspCodec)
            .try_collect()
            .await
            .unwrap();
        assert!(codec.is_empty());
    }

    #[tokio::test]
    async fn decode_invalid_header() {
        let frame = concat!(
            "Internal Whitespace: yes\r\n\r\n",
            r#"{"jsonrpc":"2.0","id":1,"result":null}"#
        );
        let res: Result<Vec<Message>, _> = FramedRead::new(frame.as_bytes(), LspCodec)
            .try_collect()
            .await;

        assert_matches!(res, Err(LspError::Headers(_)));
    }

    #[tokio::test]
    async fn decode_missing_content_length() {
        let frame = concat!(
            "Content-Type: application/vscode-jsonrpc; charset=utf8\r\n\r\n",
            r#"{"jsonrpc": "2.0", "id": 1, "result": null}"#
        );
        let res: Result<Vec<Message>, _> = FramedRead::new(frame.as_bytes(), LspCodec)
            .try_collect()
            .await;

        assert_matches!(res, Err(LspError::MissingContentLength));
    }

    #[tokio::test]
    async fn decode_invalid_content_length() {
        let frame = concat!(
            "Content-Length: not a number\r\n\r\n",
            r#"{"jsonrpc":"2.0","id":1,"result":null}"#
        );
        let res: Result<Vec<Message>, _> = FramedRead::new(frame.as_bytes(), LspCodec)
            .try_collect()
            .await;

        assert_matches!(res, Err(LspError::InvalidContentLength));
    }

    #[tokio::test]
    async fn decode_invalid_json() {
        let frame = concat!("Content-Length: 8\r\n\r\n", "not json",);
        let res: Result<Vec<Message>, _> = FramedRead::new(frame.as_bytes(), LspCodec)
            .try_collect()
            .await;

        assert_matches!(res, Err(LspError::Json(_)));
    }
}
