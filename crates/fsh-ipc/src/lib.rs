//! JSON-RPC 2.0 messaging used everywhere in Ferroshell: `fsh-ctl` → processes over named pipes,
//! and (later) shell ↔ plugins over stdio. Messages are newline-delimited JSON.

pub mod jsonrpc;
pub mod pipe;

pub use jsonrpc::{Message, Notification, Request, Response, RpcError};
