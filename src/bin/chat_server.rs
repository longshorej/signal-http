use mio::net::TcpListener;
use mio::*;
use signal_http::chat::*;
use signal_http::chat_http::*;
use signal_http::http::*;
use std::collections::HashSet;
use std::io::Error as IoError;
use std::io::ErrorKind as IoErrorKind;
use std::io::Result as IoResult;
use std::net::SocketAddr;
use std::str;
use std::usize;

const BIND_HOST: &str = "127.0.0.1";
const BIND_PORT: u16 = 8080;
const CONTACT_LIST: &str = include_str!("../../data/contacts.json");

/// Entrypoint for the chat server's binary.
///
/// This creates a `ChatServer` and parses the supplied
/// `contacts.json` file, seeding the server with valid
/// contact lists.
///
/// It then sets up an MIO event loop to process read/write
/// readiness events, using them to drive an HTTP server.
fn main() -> IoResult<()> {
    let mut chat_server = ChatServer::new();

    let contact_list_data = serde_json::from_str(CONTACT_LIST)?;

    // parse the contacts.json file, and populate the chat server's
    // contact lists.
    //
    // this could be extracted but I think it reads better inline

    if let Some(serde_json::Value::Object(contact_list_obj)) = contact_list_data {
        for (id, list_value) in contact_list_obj.into_iter() {
            if let (Ok(id), serde_json::Value::Array(list)) = (id.parse(), list_value) {
                chat_server.issue(ChatRequest::StoreContactList {
                    id,
                    list: list
                        .into_iter()
                        .filter_map(|other_id| match other_id {
                            serde_json::Value::Number(n) => n.as_u64(),
                            _ => None,
                        })
                        .collect(),
                });
            }
        }
    }

    let mut chat_http_server = ChatHttpServer::new(chat_server);

    // next, we'll setup our MIO machinery and bind to a TCP
    // socket.

    const SERVER: Token = Token(0);

    let addr = SocketAddr::new(
        BIND_HOST
            .parse()
            .map_err(|e| IoError::new(IoErrorKind::Other, e))?,
        BIND_PORT,
    );

    let server = TcpListener::bind(&addr)?;
    let poll = Poll::new()?;

    poll.register(&server, SERVER, Ready::readable(), PollOpt::edge())?;

    let mut events = Events::with_capacity(1024);
    let mut used_tokens = HashSet::new();
    let mut last_token = Token(0);
    let mut http_server =
        HttpServer::new(move |request: HttpRequest| chat_http_server.issue(request));

    println!("server listening on {}", addr);

    // we've successfully bound, so let's start the event loop,
    // forwarding the MIO events to the HTTP server

    loop {
        poll.poll(&mut events, None)?;

        for event in events.iter() {
            match event.token() {
                SERVER => loop {
                    // a connection is available, so we'll accept them until the OS
                    // indicates we'd block (edge triggered)

                    match server.accept() {
                        Ok((stream, _socket_addr)) => {
                            last_token =
                                calc_next_token(&used_tokens, last_token).ok_or_else(|| {
                                    // this is an edge case -- every token is in use, meaning
                                    // the server has ~4.2bn active connections (32bit), or
                                    // [...a very large number] of active connections (64bit)
                                    // so it's quite alright to panic..an orchestrator/supervisor
                                    // can always restart it anyways
                                    //
                                    // an alternative would be to stash this until a connection
                                    // has disconnected and thus a token has become available,
                                    // at the cost of some additional complexity

                                    IoError::new(IoErrorKind::Other, "tokens exhausted")
                                })?;

                            used_tokens.insert(last_token);

                            poll.register(&stream, last_token, Ready::all(), PollOpt::edge())?;

                            http_server.connection_accepted(Token(last_token.0), stream);
                        }

                        Err(ref e) if e.kind() == IoErrorKind::WouldBlock => {
                            break;
                        }

                        Err(e) => {
                            return Err(e);
                        }
                    }
                },

                token => {
                    // a connection is read/writable, so let the `HttpServer` know,
                    // and conditionally clean up if the connection is no longer active

                    let readiness = event.readiness();

                    if readiness.is_readable() {
                        http_server.connection_readable(token);
                    }

                    if readiness.is_writable() {
                        http_server.connection_writable(token);
                    }

                    if !http_server.is_connection_active(token) {
                        used_tokens.remove(&token);
                    }
                }
            }
        }
    }
}

fn calc_next_token(used_tokens: &HashSet<Token>, last_token: Token) -> Option<Token> {
    let mut last = last_token;

    loop {
        if last.0 == usize::MAX - 2 {
            last = Token(1);
        }

        let next = Token(last.0 + 1);

        if !used_tokens.contains(&next) {
            return Some(next);
        } else if next == last_token {
            return None;
        } else {
            last = next;
        }
    }
}
