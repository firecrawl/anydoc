mod prompts;
mod service;

pub use service::{ChatError, ChatService, ChatTurn};

#[cfg(test)]
mod service_test;
