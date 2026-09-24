//! Request/response RPC over a local named pipe (`\\.\pipe\<name>`).

use std::io::BufReader;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::{Context, anyhow};
use interprocess::local_socket::{GenericNamespaced, ListenerOptions, Stream, prelude::*};
use serde_json::Value;

use crate::jsonrpc::{Message, Request, Response, RpcError, read_message, write_message};

/// Handles one method call. Runs on the connection's thread, so it may block briefly,
/// but anything touching UI must be marshalled to the owning thread by the handler.
pub type Handler = Arc<dyn Fn(&str, Value) -> Result<Value, RpcError> + Send + Sync>;

/// Serve `name` on a background thread; each client connection gets its own thread.
pub fn serve(name: &str, handler: Handler) -> anyhow::Result<JoinHandle<()>> {
    let ns_name = name.to_ns_name::<GenericNamespaced>()?;
    let listener = ListenerOptions::new()
        .name(ns_name)
        .create_sync()
        .with_context(|| format!("creating pipe `{name}`"))?;
    let pipe_name = name.to_owned();

    let handle = std::thread::Builder::new()
        .name(format!("pipe-{pipe_name}"))
        .spawn(move || {
            for conn in listener.incoming() {
                match conn {
                    Ok(conn) => {
                        let handler = handler.clone();
                        let spawned = std::thread::Builder::new()
                            .name(format!("pipe-{pipe_name}-client"))
                            .spawn(move || serve_connection(conn, handler));
                        if let Err(e) = spawned {
                            tracing::warn!("pipe {pipe_name}: could not spawn client thread: {e}");
                        }
                    }
                    Err(e) => {
                        tracing::warn!("pipe {pipe_name}: accept failed: {e}");
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
            }
        })?;
    Ok(handle)
}

fn serve_connection(conn: Stream, handler: Handler) {
    let mut reader = BufReader::new(&conn);
    let mut writer = &conn;
    loop {
        let reply = match read_message(&mut reader) {
            Ok(None) => return,
            Err(e) => {
                tracing::debug!("pipe client dropped: {e}");
                return;
            }
            Ok(Some(Err(parse_err))) => Response::err(Value::Null, parse_err),
            Ok(Some(Ok(Message::Request(req)))) => dispatch(&handler, req),
            Ok(Some(Ok(Message::Notification(n)))) => {
                let _ = catch(&handler, &n.method, n.params);
                continue;
            }
            Ok(Some(Ok(Message::Response(_)))) => continue,
        };
        if write_message(&mut writer, &reply.into()).is_err() {
            return;
        }
    }
}

fn dispatch(handler: &Handler, req: Request) -> Response {
    match catch(handler, &req.method, req.params) {
        Ok(v) => Response::ok(req.id, v),
        Err(e) => Response::err(req.id, e),
    }
}

/// A panicking handler must not take the listener down with it.
fn catch(handler: &Handler, method: &str, params: Value) -> Result<Value, RpcError> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| handler(method, params)))
        .unwrap_or_else(|_| Err(RpcError::internal(format!("handler for `{method}` panicked"))))
}

/// Connect to `name`, send one request and wait up to `timeout` for the answer.
/// Runs on a helper thread so a hung server can't hang the caller.
pub fn call(name: &str, method: &str, params: Value, timeout: Duration) -> anyhow::Result<Value> {
    let (tx, rx) = mpsc::channel();
    let (name, method) = (name.to_owned(), method.to_owned());
    std::thread::spawn(move || {
        let _ = tx.send(call_blocking(&name, &method, params));
    });
    rx.recv_timeout(timeout)
        .map_err(|_| anyhow!("timed out waiting for a reply"))?
}

fn call_blocking(name: &str, method: &str, params: Value) -> anyhow::Result<Value> {
    let ns_name = name.to_ns_name::<GenericNamespaced>()?;
    let conn = Stream::connect(ns_name).with_context(|| format!("connecting to `{name}` (is it running?)"))?;
    let mut writer = &conn;
    write_message(&mut writer, &Request::new(1, method, params).into())?;
    let mut reader = BufReader::new(&conn);
    match read_message(&mut reader)? {
        Some(Ok(Message::Response(resp))) => Ok(resp.into_result()?),
        Some(Ok(other)) => Err(anyhow!("unexpected message: {other:?}")),
        Some(Err(e)) => Err(e.into()),
        None => Err(anyhow!("connection closed without a reply")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn serves_and_calls() {
        let name = format!("fsh-ipc-test-{}", std::process::id());
        let handler: Handler = Arc::new(|method, params| match method {
            "echo" => Ok(params),
            "boom" => panic!("handler panic"),
            other => Err(RpcError::method_not_found(other)),
        });
        serve(&name, handler).unwrap();

        let t = Duration::from_secs(5);
        assert_eq!(call(&name, "echo", json!({"x": 1}), t).unwrap(), json!({"x": 1}));
        assert!(call(&name, "nope", Value::Null, t).unwrap_err().to_string().contains("unknown method"));
        assert!(call(&name, "boom", Value::Null, t).unwrap_err().to_string().contains("panicked"));
        // The listener survives a panicking handler.
        assert_eq!(call(&name, "echo", json!(2), t).unwrap(), json!(2));
    }
}
