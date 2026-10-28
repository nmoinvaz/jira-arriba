//! Communication with Jira: the authenticated REST client and the helpers that shape its
//! requests and read its responses.

pub mod attachments;
mod client;
pub mod comments;
pub mod issues;
pub mod projects;
pub mod users;

pub use client::{Client, init_tls};
