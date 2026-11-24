//! rmcp's Streamable HTTP transport on Zwiim's egress client. rmcp builds the
//! MCP messages and headers; every byte goes out and comes back through
//! [`EgressHttpClient`], so its origin, address, redirect, credential and size
//! rules apply to all MCP traffic.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use futures::stream::{BoxStream, StreamExt};
use http::header::{ACCEPT, CONTENT_TYPE};
use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use rmcp::model::{ClientJsonRpcMessage, ClientRequest, ErrorData, ServerJsonRpcMessage};
use rmcp::transport::streamable_http_client::{
    SseError, StreamableHttpClient, StreamableHttpError, StreamableHttpPostResponse,
    StreamableHttpResponseLimits,
};
use sse_stream::{Sse, SseStream};
use url::Url;

use crate::connectors::egress::{
    Credential, EgressError, EgressHttpClient, EgressRequest, EgressResponse,
};

const SESSION_ID: &str = "mcp-session-id";
const LAST_EVENT_ID: &str = "last-event-id";
const EVENT_STREAM: &str = "text/event-stream";
const JSON: &str = "application/json";

type Error = StreamableHttpError<EgressError>;

/// One connection's HTTP client: the shared egress client and the credential
/// this connection sends. rmcp's own `auth_header` is never set.
#[derive(Clone)]
pub(super) struct McpHttpClient {
    egress: Arc<EgressHttpClient>,
    credential: Option<Credential>,
}

impl McpHttpClient {
    pub(super) fn new(egress: Arc<EgressHttpClient>, credential: Option<Credential>) -> Self {
        Self { egress, credential }
    }

    async fn send(
        &self,
        method: Method,
        uri: &str,
        mut headers: HeaderMap,
        custom_headers: HashMap<HeaderName, HeaderValue>,
        body: Option<Vec<u8>>,
    ) -> Result<EgressResponse, Error> {
        let url = Url::parse(uri)
            .map_err(|e| Error::UnexpectedServerResponse(format!("invalid URL: {e}").into()))?;
        headers.extend(custom_headers);
        self.egress
            .send(EgressRequest {
                method,
                url,
                headers,
                body: body.map(Into::into),
                credential: self.credential.clone(),
            })
            .await
            .map_err(StreamableHttpError::Client)
    }

    async fn post(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
        limits: StreamableHttpResponseLimits,
    ) -> Result<StreamableHttpPostResponse, Error> {
        let mut headers = HeaderMap::new();
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("text/event-stream, application/json"),
        );
        headers.insert(CONTENT_TYPE, HeaderValue::from_static(JSON));
        if let Some(session_id) = &session_id {
            headers.insert(SESSION_ID, header_value(session_id)?);
        }
        let body = serde_json::to_vec(&message)?;
        let response = self
            .send(Method::POST, &uri, headers, custom_headers, Some(body))
            .await?;
        let status = response.status();
        if matches!(status, StatusCode::ACCEPTED | StatusCode::NO_CONTENT) {
            return Ok(StreamableHttpPostResponse::Accepted);
        }
        if status == StatusCode::NOT_FOUND && session_id.is_some() {
            return Err(Error::SessionExpired);
        }
        let new_session = response
            .headers()
            .get(SESSION_ID)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let content_type = media_type(response.headers());
        // Notifications and replies leave nothing outstanding, so whatever
        // comes back for them is as good as 202.
        let awaits_no_response = !matches!(message, ClientJsonRpcMessage::Request(_));

        if !status.is_success() {
            let error = match content_type.as_deref() {
                Some(JSON) if !is_auth_failure(status) => response
                    .bytes_up_to(limits.max_json_response_size)
                    .await
                    .ok()
                    .and_then(|body| serde_json::from_slice::<ServerJsonRpcMessage>(&body).ok())
                    .filter(|message| matches!(message, ServerJsonRpcMessage::Error(_))),
                _ => None,
            };
            if let Some(id) = sessionless_discover_id(&message, session_id.is_some(), status) {
                // A server that predates `server/discover` may reject it with
                // a plain 4xx, or with a JSON-RPC error that can't echo the id.
                // Hand rmcp an error carrying the request's id, so it can tell
                // a legacy server and fall back to `initialize`.
                let error = match error {
                    Some(ServerJsonRpcMessage::Error(error)) => error.error,
                    _ => ErrorData::invalid_request(
                        format!("server/discover rejected with HTTP {status}"),
                        None,
                    ),
                };
                return Ok(StreamableHttpPostResponse::Json(
                    ServerJsonRpcMessage::error(error, Some(id)),
                    None,
                ));
            }
            return match error {
                Some(error) => Ok(StreamableHttpPostResponse::Json(error, new_session)),
                None => Err(Error::UnexpectedServerResponse(Cow::Owned(format!(
                    "HTTP {status}"
                )))),
            };
        }

        match content_type.as_deref() {
            Some(EVENT_STREAM) => Ok(StreamableHttpPostResponse::Sse(
                events(response, limits.max_sse_event_size),
                new_session,
            )),
            Some(JSON) => {
                let body = response
                    .bytes_up_to(limits.max_json_response_size)
                    .await
                    .map_err(StreamableHttpError::Client)?;
                match serde_json::from_slice::<ServerJsonRpcMessage>(&body) {
                    Ok(message) => Ok(StreamableHttpPostResponse::Json(message, new_session)),
                    Err(_) if awaits_no_response => Ok(StreamableHttpPostResponse::Accepted),
                    Err(e) => Err(Error::UnexpectedServerResponse(Cow::Owned(format!(
                        "the response isn't a JSON-RPC message: {e}"
                    )))),
                }
            }
            _ if awaits_no_response => Ok(StreamableHttpPostResponse::Accepted),
            _ => Err(Error::UnexpectedContentType(content_type)),
        }
    }

    async fn stream(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
        max_event: usize,
    ) -> Result<BoxStream<'static, Result<Sse, SseError>>, Error> {
        let mut headers = HeaderMap::new();
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("text/event-stream, application/json"),
        );
        if let Some(session_id) = &session_id {
            headers.insert(SESSION_ID, header_value(session_id)?);
        }
        if let Some(last_event_id) = &last_event_id {
            headers.insert(LAST_EVENT_ID, header_value(last_event_id)?);
        }
        let response = self
            .send(Method::GET, &uri, headers, custom_headers, None)
            .await?;
        let status = response.status();
        if status == StatusCode::METHOD_NOT_ALLOWED {
            return Err(Error::ServerDoesNotSupportSse);
        }
        if !status.is_success() {
            return Err(Error::UnexpectedServerResponse(Cow::Owned(format!(
                "HTTP {status}"
            ))));
        }
        match media_type(response.headers()).as_deref() {
            Some(EVENT_STREAM) => Ok(events(response, max_event)),
            other => Err(Error::UnexpectedContentType(other.map(str::to_string))),
        }
    }
}

impl StreamableHttpClient for McpHttpClient {
    type Error = EgressError;

    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        _auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, Error> {
        let limits = StreamableHttpResponseLimits::default();
        self.post(uri, message, session_id, custom_headers, limits)
            .await
    }

    async fn post_message_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        _auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
        max_sse_event_size: usize,
    ) -> Result<StreamableHttpPostResponse, Error> {
        let mut limits = StreamableHttpResponseLimits::default();
        limits.max_sse_event_size = max_sse_event_size;
        self.post(uri, message, session_id, custom_headers, limits)
            .await
    }

    async fn post_message_with_response_limits(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        _auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
        limits: StreamableHttpResponseLimits,
    ) -> Result<StreamableHttpPostResponse, Error> {
        self.post(uri, message, session_id, custom_headers, limits)
            .await
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        _auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), Error> {
        let mut headers = HeaderMap::new();
        headers.insert(SESSION_ID, header_value(&session_id)?);
        let response = self
            .send(Method::DELETE, &uri, headers, custom_headers, None)
            .await?;
        let status = response.status();
        if status.is_success() || status == StatusCode::METHOD_NOT_ALLOWED {
            Ok(())
        } else {
            Err(Error::UnexpectedServerResponse(Cow::Owned(format!(
                "HTTP {status}"
            ))))
        }
    }

    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        _auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<BoxStream<'static, Result<Sse, SseError>>, Error> {
        let max_event = StreamableHttpResponseLimits::default().max_sse_event_size;
        self.stream(uri, session_id, last_event_id, custom_headers, max_event)
            .await
    }

    async fn get_stream_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        _auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
        max_sse_event_size: usize,
    ) -> Result<BoxStream<'static, Result<Sse, SseError>>, Error> {
        self.stream(
            uri,
            session_id,
            last_event_id,
            custom_headers,
            max_sse_event_size,
        )
        .await
    }
}

fn events(response: EgressResponse, max_event: usize) -> BoxStream<'static, Result<Sse, SseError>> {
    SseStream::from_bytes_stream(response.sse(max_event)).boxed()
}

fn header_value(value: &str) -> Result<HeaderValue, Error> {
    HeaderValue::from_str(value)
        .map_err(|_| Error::UnexpectedServerResponse("invalid header value".into()))
}

/// The media type without parameters, lowercased.
fn media_type(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(CONTENT_TYPE)?.to_str().ok()?;
    Some(value.split(';').next()?.trim().to_ascii_lowercase())
}

fn is_auth_failure(status: StatusCode) -> bool {
    matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN)
}

/// The id of a `server/discover` sent outside a session that the server
/// rejected with a client error other than an auth failure.
fn sessionless_discover_id(
    message: &ClientJsonRpcMessage,
    session_attached: bool,
    status: StatusCode,
) -> Option<rmcp::model::RequestId> {
    if session_attached || !status.is_client_error() || is_auth_failure(status) {
        return None;
    }
    match message {
        ClientJsonRpcMessage::Request(request)
            if matches!(request.request, ClientRequest::DiscoverRequest(_)) =>
        {
            Some(request.id.clone())
        }
        _ => None,
    }
}
