//! Provides a simple event driven HTTP implementation
//! that is powered by MIO events.
//!
//! Simple as in the following are not supported:
//!
//! * keep-alive
//! * timeouts
//! * request size limits
//! * streaming
//! * methods beyond GET/POST
//! * fairness

use mio::net::TcpStream;
use mio::*;
use std::collections::HashMap;
use std::io::Error as IoError;
use std::io::ErrorKind as IoErrorKind;
use std::io::{Read, Result as IoResult, Write};
use std::str;
use std::usize;

/// Data is written/read from a connection's
/// socket in chunks of upto this many bytes.
const CHUNK_SIZE: usize = 8192;

/// Specifies the size of the vector used to
/// store response headers. Trade-off of
/// memory usage vs reducing reallocations.
const HEADERS_INITIAL_SIZE: usize = 8;

#[derive(Debug, PartialEq)]
pub enum BodyContent {
    Str(&'static str),
    String(String),
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum HttpMethod {
    GET,
    POST,
}

/// Represents a fully formed HTTP
/// request.
#[derive(Debug, PartialEq)]
pub struct HttpRequest<'a> {
    pub(crate) body: Option<&'a str>,
    pub(crate) headers: Vec<(&'a str, &'a str)>,
    pub(crate) method: HttpMethod,
    pub(crate) path: &'a str,
    pub(crate) version: &'a str,
}

impl<'a> HttpRequest<'a> {
    /// Get the request body, if one is present.
    pub fn body(&self) -> Option<&'a str> {
        self.body
    }

    /// Get the value of the specified header, if present.
    pub fn header<S: AsRef<str>>(&self, name: S) -> Option<&'a str> {
        let name = name.as_ref();

        for (n, v) in self.headers.iter() {
            if &name == n {
                return Some(&v);
            }
        }

        None
    }

    /// Get the method for this request
    pub fn method(&self) -> HttpMethod {
        self.method
    }

    /// Obtain the path for this request
    pub fn path(&self) -> &'a str {
        self.path
    }

    /// Obtain the version string for this request, e.g. "HTTP/1.1"
    pub fn version(&self) -> &'a str {
        self.version
    }

    /// Internal API.
    ///
    /// Parse the supplied data.
    ///
    /// `Ok(None)` means we haven't received enough data yet
    /// `Ok(Some(_))` means we've successfully parsed the request
    /// `Err(_)` means that the parsing has failed and will never succeed
    fn parse(data: &str, done: bool) -> IoResult<Option<HttpRequest>> {
        // ref: https://www.w3.org/Protocols/rfc2616/rfc2616-sec5.html

        enum State {
            ReadingRequestLine,
            ReadingHeaderLines,
            DoneReadingHeaderLines,
        }

        let mut body = "";
        let mut body_len = None;
        let mut body_start = 0;
        let mut headers: Vec<(&str, &str)> = Vec::with_capacity(HEADERS_INITIAL_SIZE);
        let mut method: Option<HttpMethod> = None;
        let mut path: Option<&str> = None;
        let mut state = State::ReadingRequestLine;
        let mut version: Option<&str> = None;

        for line in data.split("\r\n") {
            body_start += line.len() + 2; // 2 = \r\n

            match state {
                State::ReadingRequestLine => {
                    state = State::ReadingHeaderLines;

                    for (i, section) in line.split(&[' ', '\t'][..]).enumerate() {
                        match i {
                            0 => {
                                method = match section {
                                    "GET" => Some(HttpMethod::GET),
                                    "POST" => Some(HttpMethod::POST),
                                    _ => None,
                                }
                            }

                            1 => {
                                path = Some(section);
                            }

                            2 => {
                                version = Some(section);
                            }

                            _ => {}
                        }
                    }
                }

                State::ReadingHeaderLines if !line.is_empty() => {
                    let mut header_parts = line.splitn(2, ':');

                    if let Some(name) = header_parts.next() {
                        if let Some(value) = header_parts.next() {
                            let value = value.trim_start();

                            headers.push((name, value));

                            if name.to_lowercase() == "content-length" {
                                if let Ok(length) = value.parse() {
                                    body_len = Some(length);
                                }
                            }
                        }
                    }
                }

                State::ReadingHeaderLines => {
                    state = State::DoneReadingHeaderLines;

                    break;
                }

                State::DoneReadingHeaderLines => {
                    break;
                }
            }
        }

        if body_start > 0 && body_start < data.len() {
            body = &data[body_start..];
        }

        match (state, method, path, version) {
            (State::ReadingRequestLine, _, _, _) if !done => Ok(None),

            (State::ReadingHeaderLines, _, _, _) if !done => Ok(None),

            (State::ReadingRequestLine, _, _, _) => Err(IoError::new(
                IoErrorKind::InvalidInput,
                "cannot parse request",
            )),

            (State::ReadingHeaderLines, _, _, _) => Err(IoError::new(
                IoErrorKind::InvalidInput,
                "cannot parse request",
            )),

            (State::DoneReadingHeaderLines, Some(HttpMethod::GET), Some(path), Some(version)) => {
                Ok(Some(HttpRequest {
                    body: None,
                    headers,
                    method: HttpMethod::GET,
                    path,
                    version,
                }))
            }

            (State::DoneReadingHeaderLines, Some(method), Some(path), Some(version))
                if done || body_len.map_or(false, |l: usize| body.len() == l) =>
            {
                Ok(Some(HttpRequest {
                    body: Some(body),
                    headers,
                    method,
                    path,
                    version,
                }))
            }

            (State::DoneReadingHeaderLines, Some(_), Some(_), Some(_)) => Ok(None),

            (State::DoneReadingHeaderLines, _, _, _) => Err(IoError::new(
                IoErrorKind::InvalidInput,
                "cannot parse request",
            )),
        }
    }
}

/// Represents an `HttpResponse`
#[derive(Debug, PartialEq)]
pub struct HttpResponse<'a> {
    body: BodyContent,
    status: u16,
    status_text: &'static str,
    headers: Vec<(&'static str, &'static str)>,
    version: &'a str,
}

impl<'a> HttpResponse<'a> {
    /// Creates a new `HttpResponse` with the
    /// supplied fields.
    pub fn new(
        version: &'a str,
        status: u16,
        headers: &'a [(&'static str, &'static str)],
        body: BodyContent,
    ) -> Self {
        Self {
            body,
            status,
            status_text: match status {
                200 => "OK",
                400 => "Bad Request",
                404 => "Not Found",
                501 => "Not Implemented",
                _ => "",
            },
            headers: headers.to_vec(),
            version,
        }
    }

    fn unparse(&self) -> String {
        let mut resp = String::new();

        resp.push_str(self.version);
        resp.push(' ');
        resp.push_str(&self.status.to_string());
        resp.push(' ');
        resp.push_str(self.status_text);
        resp.push_str("\r\n");

        for (name, value) in self.headers.iter() {
            resp.push_str(name);
            resp.push_str(": ");
            resp.push_str(value);
            resp.push_str("\r\n");
        }

        match &self.body {
            BodyContent::Str(s) => {
                resp.push_str(&format!("Content-Length: {}\r\n", &s.len()));
            }

            BodyContent::String(s) => {
                resp.push_str(&format!("Content-Length: {}\r\n", &s.len()));
            }
        }

        resp.push_str("Connection: Close\r\n\r\n");

        match &self.body {
            BodyContent::Str(str) => {
                resp.push_str(str);
            }

            BodyContent::String(string) => {
                resp.push_str(&string);
            }
        }

        resp
    }
}

#[derive(PartialEq)]
enum ConnectionMode {
    Reading,
    Writing,
}

struct Connection {
    buffer: Vec<u8>,
    buffer_idx: usize,
    mode: ConnectionMode,
    stream: TcpStream,
}

pub struct HttpServer {
    connections: HashMap<Token, Connection>,
    handler: Box<FnMut(HttpRequest) -> HttpResponse>,
}

/// Provides a simple HTTP implementation that is driven
/// by calls to `connection_accepted`, `connection_writable`,
/// and `connection_readable`.
impl HttpServer {
    /// Creates a new `HttpServer` that passes incoming requests
    /// to the suplied handler and responds with the produced
    /// response.
    pub fn new<F: FnMut(HttpRequest) -> HttpResponse>(handler: F) -> Self
    where
        F: 'static,
    {
        Self {
            connections: HashMap::new(),
            handler: Box::new(handler),
        }
    }

    /// A new connection was accepted and will now be managed by this
    /// instance.
    ///
    /// The connection's status can be queried by using the `is_connection_active`
    /// method.
    pub fn connection_accepted(&mut self, token: Token, stream: TcpStream) {
        self.connections.insert(
            token,
            Connection {
                buffer: Vec::new(),
                buffer_idx: 0,
                mode: ConnectionMode::Reading,
                stream,
            },
        );
    }

    /// Signals to the server that data can now be written
    /// to the specified connection.
    pub fn connection_writable(&mut self, token: Token) {
        if let Some(cx) = self.connections.get_mut(&token) {
            if cx.mode == ConnectionMode::Writing && Self::perform_writes(cx) {
                self.connections.remove(&token);
            }
        }
    }

    /// Signals to the server that data can now be read
    /// from the connection.
    pub fn connection_readable(&mut self, token: Token) {
        if let Some(cx) = self.connections.get_mut(&token) {
            if let ConnectionMode::Reading { .. } = cx.mode {
                match Self::perform_reads(cx) {
                    Ok(done) => {
                        if done {
                            cx.mode = ConnectionMode::Writing;
                        }

                        Self::try_parse_request(&mut self.handler, cx);

                        if cx.mode == ConnectionMode::Writing && Self::perform_writes(cx) {
                            self.connections.remove(&token);
                        }
                    }

                    Err(_) => {
                        cx.mode = ConnectionMode::Writing;
                        self.connections.remove(&token);
                    }
                }
            }
        }
    }

    /// Determines if the connection is active.
    pub fn is_connection_active(&self, token: Token) -> bool {
        self.connections.contains_key(&token)
    }

    /// Internal API.
    ///
    /// Reads all data available from the connection,
    /// returning whether the read side has been
    /// closed, i.e. no more data will be available.
    ///
    /// This should only be called if it's known that
    /// data is available -- i.e. an MIO event has
    /// been received.
    fn perform_reads(cx: &mut Connection) -> IoResult<bool> {
        loop {
            if cx.buffer.len() - cx.buffer_idx == 0 {
                cx.buffer.resize(cx.buffer.len() + CHUNK_SIZE, 0);
            }

            match cx.stream.read(&mut cx.buffer[cx.buffer_idx..]) {
                Ok(0) => {
                    return Ok(true);
                }

                Ok(bytes_read) => {
                    cx.buffer_idx += bytes_read;
                }

                Err(ref e) if e.kind() == IoErrorKind::WouldBlock => {
                    break;
                }

                Err(e) => {
                    return Err(e);
                }
            }
        }

        Ok(true)
    }

    /// Internal API.
    ///
    /// Writes all data available until the connection
    /// indicates it would block, and returns whether
    /// all data has infact been written.
    fn perform_writes(cx: &mut Connection) -> bool {
        while cx.buffer_idx < cx.buffer.len() {
            match cx.stream.write(&cx.buffer[cx.buffer_idx..]) {
                Ok(0) => {
                    return true;
                }

                Ok(bytes_written) => {
                    cx.buffer_idx += bytes_written;
                }

                Err(ref e) if e.kind() == IoErrorKind::WouldBlock => {
                    return false;
                }

                Err(_) => {
                    return true;
                }
            }
        }

        true
    }

    /// Internal API.
    ///
    /// Attempt to parse the current buffer contents.
    ///
    /// If successful, the handler will be invoked with
    /// the request and must produce a response. The
    /// connection will then be switched into writing
    /// mode and begin writing data.
    fn try_parse_request(handler: &mut FnMut(HttpRequest) -> HttpResponse, cx: &mut Connection) {
        if let Ok(req) = str::from_utf8(&cx.buffer[0..cx.buffer_idx]) {
            match HttpRequest::parse(req, cx.mode == ConnectionMode::Writing) {
                Ok(Some(req)) => {
                    let response = handler(req);

                    cx.buffer = response.unparse().as_bytes().to_vec();
                    cx.buffer_idx = 0;
                    cx.mode = ConnectionMode::Writing;
                }

                Ok(None) => {
                    // not ready yet
                }

                Err(_) => {
                    let response = HttpResponse {
                        body: BodyContent::Str(""),
                        status: 400,
                        status_text: "Bad Request",
                        headers: Vec::new(),
                        version: "HTTP/1.1",
                    };

                    cx.buffer = response.unparse().as_bytes().to_vec();
                    cx.buffer_idx = 0;
                    cx.mode = ConnectionMode::Writing;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::http::*;

    #[test]
    fn test_invalid() {
        assert!(HttpRequest::parse("", true).is_err(),);

        assert!(HttpRequest::parse("GET /chats\r\n", false).is_err(),);
    }

    #[test]
    fn test_incomplete() {
        assert_eq!(HttpRequest::parse("", false).unwrap(), None)
    }

    #[test]
    fn test_http_request_parse_get() {
        assert_eq!(
            HttpRequest::parse("GET /chats/1/messages HTTP/1.0\r\nMy-Header: hello!\r\nMy-Other-Header: goodbye!\r\n\r\n", true)
                .unwrap(),

            Some(HttpRequest {
                body: None,
                headers: vec![
                    ("My-Header", "hello!"),
                    ("My-Other-Header", "goodbye!")
                ],
                method: HttpMethod::GET,
                path: "/chats/1/messages",
                version: "HTTP/1.0"
            })
        );
    }

    #[test]
    fn test_http_request_parse_post() {
        assert_eq!(
            HttpRequest::parse("POST /chats/1/messages HTTP/1.1\r\n\r\ntest\r\n", true).unwrap(),
            Some(HttpRequest {
                body: Some("test\r\n"),
                headers: Vec::new(),
                method: HttpMethod::POST,
                path: "/chats/1/messages",
                version: "HTTP/1.1"
            })
        );
    }
}
