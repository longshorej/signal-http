# signal-http

This project implements an asynchronous non-blocking HTTP webserver
for chatting. It's implemented directly ontop of NIO to illustrate
the basics of an HTTP server implementation.

## Project Setup

You'll need `cargo` - see [https://rustup.rs/](https://rustup.rs/). With it
installed, you can build a release binary as follows:

```bash
cargo build --release
```

The binary will be in `target/release/chat_server`. Launch this and it will bind to port *8080*.

## Using the Server

Once you've started the webserver, we can start issuing requests.

First, let's create a chat:

```bash
curl -i -XPOST http://127.0.0.1:8080/chats --data '{
  "id": 1,
  "participantIds": [51201, 22307] 
}'
```

which should respond with:

```text
HTTP/1.1 200 OK
Content-Type: text/plain
Content-Length: 29
Connection: Close

The supplied chat was created
```

Next, we'll send a message to this chat:

```bash
curl -i -XPOST http://127.0.0.1:8080/chats/1/messages --data '{
  "id": "a3113eca-bb08-4861-97bb-f5ba2535529e", 
  "timestamp": 1000, 
  "message": "Hello there!", 
  "sourceUserId": 51201, 
  "destinationUserId": 22307 
}'
```

which yields:

```text
HTTP/1.1 200 OK
Content-Type: text/plain
Content-Length: 42
Connection: Close

The supplied message was added to the chat
```

Now, let's retrieve the chats for user 51201:

```bash
curl -i -XGET http://127.0.0.1:8080/chats?userId=51201
```

yielding:

```text
HTTP/1.1 200 OK
Content-Type: application/json
Content-Length: 41
Connection: Close

[{"id":1,"participantIds":[51201,22307]}]
```

...and the chat's content:

```bash
curl -i -XGET http://127.0.0.1:8080/chats/1/messages
```

resulting in:

```text
HTTP/1.1 200 OK
Content-Type: application/json
Content-Length: 136
Connection: Close

[{"id":"a3113eca-bb08-4861-97bb-f5ba2535529e","timestamp":1000,"message":"Hello there!","sourceUserId":51201,"destinationUserId":22307}]
```

## Design Info / Process

The chat server was built in a few separate modules, allowing me to defer
needing to implement the HTTP and MIO requirements immediately.

1) `chat` was the first module I implemented. Being that HTTP
is a request/response protocol, I chose to model a similar abstraction
for this as well. It allowed me to implement all of the required
functionality for creating chats, chat messages, etc, without immediately
focusing on the translation to and from HTTP. A side-effect of this is that
it becomes easily testable. This code is in `src/chat.rs`.

2) With this in place, I started to think about translating these domain
request and responses into HTTP. You can find the simple data structures for
HTTP requests and responses in `src/http.rs`. With these data structures,
I implemented the `chat_http` module that provides facilities for translating
chat requests into HTTP requests, and chat responses into HTTP responses. This
translation layer is in `src/chat_http.rs`.

3) Then, I started thinking about how to parse incoming requests, and "unparse"
outgoing responses. You can find this in `src/http.rs` -- `HttpRequest::parse`
and `HttpResponse::unparse`. No streaming here, but it does the job given
the time constraints.

4) At this point, I needed to think about implementing the state machine that
is the HTTP server, and the MIO loop to drive it. You can find the setup code
for MIO in the program's entrypoint, `src/bin/chat_server.rs`. It's a
single-threaded event loop that receives readiness events from MIO and forwards
them to the `HttpServer` implementation. This must be constructed with a
request handler -- an `FnMut` that turns `HttpRequest`s into `HttpResponse`s.
This makes it trivial to plugin the `chat_http::ChatHttpServer` logic. 

## Developer Tips

### Running Tests

```bash
cargo test
```

### Formatting Code

```bash
cargo fmt
```

### Code Linting

```bash
cargo clippy
```

## Author

Jason Longshore <longshorej@gmail.com>
