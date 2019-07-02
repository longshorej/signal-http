//! Contains the primary logic for the chat server,
//! which has a pure domain logic implementation,
//! `ChatServer`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::str;
use std::usize;

/// Id type for chats, messages, users
pub type Id = u64;

/// Response representation of a chat
#[derive(Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Chat {
    pub(crate) id: Id,
    pub(crate) participant_ids: [Id; 2],
}

/// Response representation of a chat message
#[derive(Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    pub(crate) id: String,
    pub(crate) timestamp: u64,
    pub(crate) message: String,
    pub(crate) source_user_id: Id,
    pub(crate) destination_user_id: Id,
}

/// Contains request messages for the chat request-response
/// protocol.
pub enum ChatRequest {
    CreateChat {
        id: Id,
        participant_ids: [Id; 2],
    },

    AddMessage {
        id: String,
        chat_id: Id,
        source_user_id: Id,
        destination_user_id: Id,
        timestamp: u64,
        message: String,
    },

    ListChats {
        user_id: Id,
    },

    ListChat {
        id: Id,
    },

    StoreContactList {
        id: Id,
        list: Vec<Id>,
    },
}

/// Contains response messages for the chat request-response
/// protocol.
#[derive(Debug, PartialEq)]
pub enum ChatResponse<'a> {
    ChatCreated,
    ChatAlreadyExists,
    ChatParsingError,
    ChatValidationError,
    ChatListed { messages: &'a [ChatMessage] },
    ChatsListed { chats: Vec<Chat> },
    ContactListStored,
    MessageAdded,
    MessageParsingError,
    UnknownChat,
}

/// Implements the "domain logic" for the chat server,
/// which receives `ChatRequest`s and turns them into
/// `ChatResponse`s, mutating its state whilst doing so.
#[derive(Default)]
pub struct ChatServer {
    chats: HashMap<Id, StoredChat>,
    chats_by_user_id: HashMap<Id, Vec<ChatRef>>,
    contact_lists: HashMap<Id, Vec<Id>>,
}

impl ChatServer {
    /// Creates a new chat server
    pub fn new() -> Self {
        Self {
            chats: HashMap::new(),
            chats_by_user_id: HashMap::new(),
            contact_lists: HashMap::new(),
        }
    }

    /// Issue a domain-specific request against this chat
    /// server, returning a domain-specific response.
    pub fn issue(&mut self, command: ChatRequest) -> ChatResponse {
        match command {
            ChatRequest::CreateChat {
                id,
                participant_ids,
            } => {
                if self.chats.contains_key(&id)
                    || self
                        .chat_id(participant_ids[0], participant_ids[1])
                        .is_some()
                {
                    ChatResponse::ChatAlreadyExists
                } else if !self
                    .contact_lists
                    .get(&participant_ids[0])
                    .map_or(false, |list| list.contains(&participant_ids[1]))
                    || !self
                        .contact_lists
                        .get(&participant_ids[1])
                        .map_or(false, |list| list.contains(&participant_ids[0]))
                {
                    ChatResponse::ChatValidationError
                } else {
                    self.chats.insert(
                        id,
                        StoredChat {
                            participant_ids,
                            messages: Vec::new(),
                        },
                    );

                    self.chats_by_user_id
                        .entry(participant_ids[0])
                        .or_insert_with(Vec::new)
                        .push(ChatRef {
                            id,
                            destination_user_id: participant_ids[1],
                        });

                    self.chats_by_user_id
                        .entry(participant_ids[1])
                        .or_insert_with(Vec::new)
                        .push(ChatRef {
                            id,
                            destination_user_id: participant_ids[0],
                        });

                    ChatResponse::ChatCreated
                }
            }

            ChatRequest::AddMessage {
                id,
                chat_id,
                source_user_id,
                destination_user_id,
                timestamp,
                message,
            } => self
                .chat_id(source_user_id, destination_user_id)
                .filter(|other_chat_id| chat_id == *other_chat_id)
                .and_then(|chat_id| self.chats.get_mut(&chat_id))
                .map_or(ChatResponse::UnknownChat, |chat| {
                    chat.insert(id, source_user_id, destination_user_id, timestamp, message);

                    ChatResponse::MessageAdded
                }),

            ChatRequest::ListChats { user_id } => {
                let chat_refs = self.chats_by_user_id.get(&user_id);

                match chat_refs {
                    Some(rs) => {
                        let mut chats = Vec::with_capacity(rs.len());

                        for r in rs {
                            if let Some(c) = self.chats.get(&r.id) {
                                chats.push(Chat {
                                    id: r.id,
                                    participant_ids: c.participant_ids,
                                });
                            }
                        }

                        ChatResponse::ChatsListed { chats }
                    }

                    None => ChatResponse::ChatsListed { chats: Vec::new() },
                }
            }

            ChatRequest::ListChat { id } => match self.chats.get(&id) {
                Some(chat) => ChatResponse::ChatListed {
                    messages: &chat.messages,
                },

                None => ChatResponse::UnknownChat,
            },

            ChatRequest::StoreContactList { id, list } => {
                self.contact_lists.insert(id, list);

                ChatResponse::ContactListStored
            }
        }
    }

    /// Internal API.
    ///
    /// Given the ID of two users, determines the ID of the chat
    /// between them if there is one.
    fn chat_id(&mut self, source_user_id: Id, destination_user_id: Id) -> Option<Id> {
        self.chats_by_user_id
            .get(&source_user_id)
            .and_then(|chats| {
                chats
                    .iter()
                    .find(|r| r.destination_user_id == destination_user_id)
            })
            .map(|chat_ref| chat_ref.id)
    }
}

/// Internal API.
///
/// The in-memory representation of a chat, which consists of
/// a sorted vector of `ChatMessage`s and an array of the
/// participants' ids.
#[derive(Debug, PartialEq)]
struct StoredChat {
    participant_ids: [Id; 2],
    messages: Vec<ChatMessage>,
}

impl StoredChat {
    /// Internal API.
    ///
    /// Insert a new chat message into this instance. This uses
    /// a simple algorithm that scans from the end of the vector.
    fn insert(
        &mut self,
        id: String,
        source_user_id: Id,
        destination_user_id: Id,
        timestamp: u64,
        message: String,
    ) {
        // simple algorithm scans from the end of the vector, finding
        // the spot to insert at. this is optimized for when received
        // messages are typically newer than previously received, or
        // at least relatively recent

        let chat_message = ChatMessage {
            id,
            timestamp,
            message,
            source_user_id,
            destination_user_id,
        };

        let len = self.messages.len();
        let messages = self.messages.as_slice();
        let mut i = len;

        while i > 0 && messages[i - 1].timestamp > timestamp {
            i -= 1;
        }

        if i == len {
            self.messages.push(chat_message);
        } else {
            self.messages.insert(i, chat_message);
        }
    }
}

/// Internal API.
///
/// Representation of available chats for a particular user,
/// including the chat id and the other participant's id.
struct ChatRef {
    id: Id,
    destination_user_id: Id,
}

#[cfg(test)]
mod tests {
    use crate::chat::*;

    #[test]
    fn test_chat_server() {
        let mut server = ChatServer::new();

        // first, given that there are no loaded contact lists,
        // we assert that we can't create a chat

        assert_eq!(
            server.issue(ChatRequest::CreateChat {
                id: 1,
                participant_ids: [1, 2]
            }),
            ChatResponse::ChatValidationError
        );

        // then, we'll load a contact list and assert that we
        // still cannot create a chat (must be symmetric)

        assert_eq!(
            server.issue(ChatRequest::StoreContactList {
                id: 1,
                list: vec![1, 2]
            }),
            ChatResponse::ContactListStored
        );

        assert_eq!(
            server.issue(ChatRequest::CreateChat {
                id: 1,
                participant_ids: [1, 2]
            }),
            ChatResponse::ChatValidationError
        );

        // next, let's setup the other side and assert that we
        // can now create a contact list

        assert_eq!(
            server.issue(ChatRequest::StoreContactList {
                id: 2,
                list: vec![2, 1]
            }),
            ChatResponse::ContactListStored
        );

        assert_eq!(
            server.issue(ChatRequest::CreateChat {
                id: 1,
                participant_ids: [1, 2]
            }),
            ChatResponse::ChatCreated
        );

        // the chat should be visible for both users

        assert_eq!(
            server.issue(ChatRequest::ListChats { user_id: 1 }),
            ChatResponse::ChatsListed {
                chats: vec![Chat {
                    id: 1,
                    participant_ids: [1, 2]
                }]
            }
        );

        assert_eq!(
            server.issue(ChatRequest::ListChats { user_id: 2 }),
            ChatResponse::ChatsListed {
                chats: vec![Chat {
                    id: 1,
                    participant_ids: [1, 2]
                }]
            }
        );

        // and visible by its id (no messages yet)

        assert_eq!(
            server.issue(ChatRequest::ListChat { id: 1 }),
            ChatResponse::ChatListed {
                messages: &Vec::new()
            }
        );

        // when we add messages, they should be visible
        // and ordered

        assert_eq!(
            server.issue(ChatRequest::AddMessage {
                id: "aed531ba-7a41-46dd-8e5d-9a5f7c16bfee".to_string(),
                chat_id: 1,
                source_user_id: 1,
                destination_user_id: 2,
                timestamp: 0,
                message: "zero".to_string()
            }),
            ChatResponse::MessageAdded
        );

        assert_eq!(
            server.issue(ChatRequest::AddMessage {
                id: "b213468f-eed5-4119-be6c-bb780120502a".to_string(),
                chat_id: 1,
                source_user_id: 2,
                destination_user_id: 1,
                timestamp: 4,
                message: "four".to_string()
            }),
            ChatResponse::MessageAdded
        );

        assert_eq!(
            server.issue(ChatRequest::AddMessage {
                id: "16cce9af-4086-4219-a54b-8b082b3c42ef".to_string(),
                chat_id: 1,
                source_user_id: 1,
                destination_user_id: 2,
                timestamp: 3,
                message: "three".to_string()
            }),
            ChatResponse::MessageAdded
        );

        assert_eq!(
            server.issue(ChatRequest::ListChat { id: 1 }),
            ChatResponse::ChatListed {
                messages: &[
                    ChatMessage {
                        id: "aed531ba-7a41-46dd-8e5d-9a5f7c16bfee".to_string(),
                        timestamp: 0,
                        message: "zero".to_string(),
                        source_user_id: 1,
                        destination_user_id: 2
                    },
                    ChatMessage {
                        id: "16cce9af-4086-4219-a54b-8b082b3c42ef".to_string(),
                        timestamp: 3,
                        message: "three".to_string(),
                        source_user_id: 1,
                        destination_user_id: 2
                    },
                    ChatMessage {
                        id: "b213468f-eed5-4119-be6c-bb780120502a".to_string(),
                        timestamp: 4,
                        message: "four".to_string(),
                        source_user_id: 2,
                        destination_user_id: 1
                    }
                ]
            }
        );
    }

    #[test]
    fn test_chart_insert() {
        let mut chat = StoredChat {
            participant_ids: [0, 1],
            messages: Vec::new(),
        };

        let data = [
            (1, "test1"),
            (4, "test2"),
            (3, "test3"),
            (5, "test4"),
            (0, "test5"),
            (6, "test6"),
            (2, "test7"),
            (9, "test8"),
            (0, "test9"),
            (9, "test10"),
        ];

        for (timestamp, message) in data.iter() {
            chat.insert("".to_string(), 0, 0, *timestamp, message.to_string());
        }

        assert_eq!(
            chat.messages
                .iter()
                .map(|msg| msg.message.as_str())
                .collect::<Vec<_>>(),
            vec![
                "test5", "test9", "test1", "test7", "test3", "test2", "test4", "test6", "test8",
                "test10"
            ]
        );
    }
}
