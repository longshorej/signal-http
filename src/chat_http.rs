//! Provides a translation layer, translating `HttpRequest`s
//! into `ChatRequest`s, and `ChatResponse`s into `HttpResponse`s.

use crate::chat::*;
use crate::http::*;

/// Wraps a `ChatServer` and translates its protocol
/// to HTTP. In other words, turns HTTP requests into
/// HTTP responses using the underlying `ChatServer`.
pub struct ChatHttpServer {
    server: ChatServer,
}

impl ChatHttpServer {
    /// Create a new `ChatHttpServer` that can be used
    /// to transform requests into responses via the
    /// provided `handle` method.
    pub fn new(server: ChatServer) -> Self {
        Self { server }
    }

    /// Process the supplied `HttpRequest`, returning an appropriate `HttpResponse`.
    pub fn issue<'a>(&mut self, request: HttpRequest<'a>) -> HttpResponse<'a> {
        let mut parts = request.path().split_terminator('/');

        let _ = parts.next(); // skip over the initial empty component (pre-leading slash)

        match (request.method(), parts.next(), parts.next(), parts.next()) {
            (HttpMethod::POST, Some("chats"), None, None) => Self::encode(
                &request,
                match serde_json::from_str::<Chat>(request.body().unwrap_or_default()) {
                    Ok(chat) => self.server.issue(ChatRequest::CreateChat {
                        id: chat.id,
                        participant_ids: chat.participant_ids,
                    }),

                    Err(_) => ChatResponse::ChatParsingError,
                },
            ),

            (HttpMethod::POST, Some("chats"), Some(chat_id), Some("messages")) => Self::encode(
                &request,
                match (
                    chat_id.parse(),
                    serde_json::from_str::<ChatMessage>(request.body().unwrap_or_default()),
                ) {
                    (Ok(chat_id), Ok(message)) => self.server.issue(ChatRequest::AddMessage {
                        id: message.id,
                        chat_id,
                        source_user_id: message.source_user_id,
                        destination_user_id: message.destination_user_id,
                        timestamp: message.timestamp,
                        message: message.message,
                    }),

                    (_, Err(_)) => ChatResponse::MessageParsingError,

                    _ => ChatResponse::UnknownChat,
                },
            ),

            (HttpMethod::GET, Some(path), None, None) if path.starts_with("chats?userId=") => {
                let user_id = &path["chats?userId=".len()..];

                Self::encode(
                    &request,
                    match user_id.parse() {
                        Ok(user_id) => self.server.issue(ChatRequest::ListChats { user_id }),

                        Err(_) => ChatResponse::ChatsListed { chats: Vec::new() },
                    },
                )
            }

            (HttpMethod::GET, Some("chats"), Some(chat_id), Some("messages")) => Self::encode(
                &request,
                match chat_id.parse() {
                    Ok(id) => self.server.issue(ChatRequest::ListChat { id }),

                    Err(_) => ChatResponse::UnknownChat,
                },
            ),

            _ => HttpResponse::new(
                request.version(),
                404,
                &[("Content-Type", "text/plain")],
                BodyContent::Str("The route is unknown"),
            ),
        }
    }

    /// Internal API.
    ///
    /// Encodes the given `ChatResponse`, returning an appropriate
    /// `HttpResponse`.
    fn encode<'a>(request: &HttpRequest<'a>, resp: ChatResponse) -> HttpResponse<'a> {
        match resp {
            ChatResponse::UnknownChat => HttpResponse::new(
                request.version(),
                404,
                &[("Content-Type", "text/plain")],
                BodyContent::Str("A chat with the provided id does not exist"),
            ),

            ChatResponse::ChatAlreadyExists => HttpResponse::new(
                request.version(),
                400,
                &[("Content-Type", "text/plain")],
                BodyContent::Str("The supplied chat was not created because one already exists"),
            ),

            ChatResponse::ChatParsingError => HttpResponse::new(
                request.version(),
                400,
                &[("Content-Type", "text/plain")],
                BodyContent::Str("The supplied chat was not created due to a parsing error"),
            ),

            ChatResponse::ChatValidationError => HttpResponse::new(
                request.version(),
                400,
                &[("Content-Type", "text/plain")],
                BodyContent::Str("The supplied chat was not created due to a validation error"),
            ),

            ChatResponse::ChatCreated => HttpResponse::new(
                request.version(),
                200,
                &[("Content-Type", "text/plain")],
                BodyContent::Str("The supplied chat was created"),
            ),

            ChatResponse::ContactListStored => HttpResponse::new(
                request.version(),
                501,
                &[("Content-Type", "text/plain")],
                BodyContent::Str("Contact lists cannot be managed over HTTP"),
            ),

            ChatResponse::ChatListed { messages } => HttpResponse::new(
                request.version(),
                200,
                &[("Content-Type", "application/json")],
                BodyContent::String(
                    serde_json::to_string(&messages).unwrap_or_else(|_| "[]".to_string()),
                ),
            ),

            ChatResponse::ChatsListed { chats } => HttpResponse::new(
                request.version(),
                200,
                &[("Content-Type", "application/json")],
                BodyContent::String(
                    serde_json::to_string(&chats).unwrap_or_else(|_| "[]".to_string()),
                ),
            ),

            ChatResponse::MessageAdded => HttpResponse::new(
                request.version(),
                200,
                &[("Content-Type", "text/plain")],
                BodyContent::Str("The supplied message was added to the chat"),
            ),

            ChatResponse::MessageParsingError => HttpResponse::new(
                request.version(),
                400,
                &[("Content-Type", "text/plain")],
                BodyContent::Str(
                    "The supplied message was not added to the chat due to a parsing error",
                ),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::chat::*;
    use crate::chat_http::*;

    #[test]
    fn test_chat_http_server() {
        let mut chat_server = ChatServer::new();

        assert_eq!(
            chat_server.issue(ChatRequest::StoreContactList {
                id: 1,
                list: vec![1, 2]
            }),
            ChatResponse::ContactListStored
        );

        assert_eq!(
            chat_server.issue(ChatRequest::StoreContactList {
                id: 2,
                list: vec![1, 2]
            }),
            ChatResponse::ContactListStored
        );

        let mut server = ChatHttpServer::new(chat_server);

        // test 404

        assert_eq!(
            server.issue(HttpRequest {
                body: None,
                headers: Vec::new(),
                method: HttpMethod::GET,
                path: "/nope",
                version: "HTTP/1.1"
            }),
            HttpResponse::new(
                "HTTP/1.1",
                404,
                &[("Content-Type", "text/plain")],
                BodyContent::Str("The route is unknown")
            )
        );

        // create an unparseable chat

        assert_eq!(
            server.issue(HttpRequest {
                body: Some("[]"),
                headers: vec![("Content-Type", "application/json")],
                method: HttpMethod::POST,
                path: "/chats",
                version: "HTTP/1.1"
            }),
            HttpResponse::new(
                "HTTP/1.1",
                400,
                &[("Content-Type", "text/plain")],
                BodyContent::Str("The supplied chat was not created due to a parsing error")
            )
        );

        // create a chat between invalid contacts

        assert_eq!(
            server.issue(HttpRequest {
                body: Some("{ \"id\": 1, \"participantIds\": [2, 3] }"),
                headers: vec![("Content-Type", "application/json")],
                method: HttpMethod::POST,
                path: "/chats",
                version: "HTTP/1.1"
            }),
            HttpResponse::new(
                "HTTP/1.1",
                400,
                &[("Content-Type", "text/plain")],
                BodyContent::Str("The supplied chat was not created due to a validation error")
            )
        );

        // create a valid chat

        assert_eq!(
            server.issue(HttpRequest {
                body: Some("{ \"id\": 1, \"participantIds\": [1, 2] }"),
                headers: vec![("Content-Type", "application/json")],
                method: HttpMethod::POST,
                path: "/chats",
                version: "HTTP/1.1"
            }),
            HttpResponse::new(
                "HTTP/1.1",
                200,
                &[("Content-Type", "text/plain")],
                BodyContent::Str("The supplied chat was created")
            )
        );

        // create an unparseable chat message

        assert_eq!(
            server.issue(HttpRequest {
                body: Some("[]"),
                headers: vec![("Content-Type", "application/json")],
                method: HttpMethod::POST,
                path: "/chats/1/messages",
                version: "HTTP/1.1"
            }),
            HttpResponse::new(
                "HTTP/1.1",
                400,
                &[("Content-Type", "text/plain")],
                BodyContent::Str(
                    "The supplied message was not added to the chat due to a parsing error"
                )
            )
        );

        // create a chat message for unknown chat

        assert_eq!(
            server.issue(HttpRequest {
                body: Some("{ \"id\": \"a15e7d99-7d6d-490b-acee-ed0356c2a9a9\", \"timestamp\": 0, \"message\": \"test\", \"sourceUserId\": 1, \"destinationUserId\": 2 }"),
                headers: vec![("Content-Type", "application/json")],
                method: HttpMethod::POST,
                path: "/chats/2/messages",
                version: "HTTP/1.1"
            }),

            HttpResponse::new(
                "HTTP/1.1",
                404,
                &[("Content-Type", "text/plain")],
                BodyContent::Str("A chat with the provided id does not exist")
            )
        );

        // create a chat message for known chat, wrong participants

        assert_eq!(
            server.issue(HttpRequest {
                body: Some("{ \"id\": \"d8ae0e72-8dcd-4660-9aa6-68c1df3cdd38\", \"timestamp\": 0, \"message\": \"test\", \"sourceUserId\": 3, \"destinationUserId\": 2 }"),
                headers: vec![("Content-Type", "application/json")],
                method: HttpMethod::POST,
                path: "/chats/1/messages",
                version: "HTTP/1.1"
            }),

            HttpResponse::new(
                "HTTP/1.1",
                404,
                &[("Content-Type", "text/plain")],
                BodyContent::Str("A chat with the provided id does not exist")
            )
        );

        assert_eq!(
            server.issue(HttpRequest {
                body: Some("{ \"id\": \"d8ae0e72-8dcd-4660-9aa6-68c1df3cdd38\", \"timestamp\": 0, \"message\": \"test\", \"sourceUserId\": 1, \"destinationUserId\": 3 }"),
                headers: vec![("Content-Type", "application/json")],
                method: HttpMethod::POST,
                path: "/chats/1/messages",
                version: "HTTP/1.1"
            }),

            HttpResponse::new(
                "HTTP/1.1",
                404,
                &[("Content-Type", "text/plain")],
                BodyContent::Str("A chat with the provided id does not exist")
            )
        );

        // create a valid chat message

        assert_eq!(
            server.issue(HttpRequest {
                body: Some("{ \"id\": \"ed27b825-1ed2-4cde-9895-93d8bdcf0984\", \"timestamp\": 0, \"message\": \"test\", \"sourceUserId\": 1, \"destinationUserId\": 2 }"),
                headers: vec![("Content-Type", "application/json")],
                method: HttpMethod::POST,
                path: "/chats/1/messages",
                version: "HTTP/1.1"
            }),

            HttpResponse::new(
                "HTTP/1.1",
                200,
                &[("Content-Type", "text/plain")],
                BodyContent::Str("The supplied message was added to the chat")
            )
        );

        // get chats by user id

        assert_eq!(
            server.issue(HttpRequest {
                body: None,
                headers: vec![],
                method: HttpMethod::GET,
                path: "/chats?userId=1",
                version: "HTTP/1.1"
            }),
            HttpResponse::new(
                "HTTP/1.1",
                200,
                &[("Content-Type", "application/json")],
                BodyContent::String("[{\"id\":1,\"participantIds\":[1,2]}]".to_string())
            )
        );

        assert_eq!(
            server.issue(HttpRequest {
                body: None,
                headers: vec![],
                method: HttpMethod::GET,
                path: "/chats?userId=2",
                version: "HTTP/1.1"
            }),
            HttpResponse::new(
                "HTTP/1.1",
                200,
                &[("Content-Type", "application/json")],
                BodyContent::String("[{\"id\":1,\"participantIds\":[1,2]}]".to_string())
            )
        );

        assert_eq!(
            server.issue(HttpRequest {
                body: None,
                headers: vec![],
                method: HttpMethod::GET,
                path: "/chats?userId=3",
                version: "HTTP/1.1"
            }),
            HttpResponse::new(
                "HTTP/1.1",
                200,
                &[("Content-Type", "application/json")],
                BodyContent::String("[]".to_string())
            )
        );

        // get known chat messages

        assert_eq!(
            server.issue(HttpRequest {
                body: None,
                headers: vec![],
                method: HttpMethod::GET,
                path: "/chats/1/messages",
                version: "HTTP/1.1"
            }),

            HttpResponse::new(
                "HTTP/1.1",
                200,
                &[("Content-Type", "application/json")],
                BodyContent::String("[{\"id\":\"ed27b825-1ed2-4cde-9895-93d8bdcf0984\",\"timestamp\":0,\"message\":\"test\",\"sourceUserId\":1,\"destinationUserId\":2}]".to_string())
            )
        );

        // get unknown chat messages

        assert_eq!(
            server.issue(HttpRequest {
                body: None,
                headers: vec![],
                method: HttpMethod::GET,
                path: "/chats/2/messages",
                version: "HTTP/1.1"
            }),
            HttpResponse::new(
                "HTTP/1.1",
                404,
                &[("Content-Type", "text/plain")],
                BodyContent::Str("A chat with the provided id does not exist")
            )
        );
    }

}
