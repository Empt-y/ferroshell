//! Hang detection. The shell answers `ping` on its UI thread, so a reply proves the event
//! loop is alive. Several missed pings in a row mean it's frozen: kill it and let the
//! crash policy restart it.

use std::sync::Arc;
use std::time::Duration;

use fsh_common::pipes;
use serde_json::Value;

use crate::supervisor::Supervisor;

const INTERVAL: Duration = Duration::from_secs(5);
const TIMEOUT: Duration = Duration::from_secs(4);
/// Give a starting shell time to build its panels before judging it.
const GRACE: Duration = Duration::from_secs(20);
const MAX_MISSES: u32 = 3;

pub fn spawn(sup: Arc<Supervisor>) -> std::io::Result<()> {
    std::thread::Builder::new().name("hang-watchdog".into()).spawn(move || {
        let mut misses = 0;
        let mut watched_pid = 0;
        loop {
            std::thread::sleep(INTERVAL);
            let Some((pid, uptime)) = sup.running_child() else {
                misses = 0;
                continue;
            };
            if pid != watched_pid {
                watched_pid = pid;
                misses = 0;
            }
            if uptime < GRACE {
                continue;
            }
            match fsh_ipc::pipe::call(pipes::SHELL, "ping", Value::Null, TIMEOUT) {
                Ok(_) => misses = 0,
                Err(e) => {
                    misses += 1;
                    tracing::warn!("shell (pid {pid}) missed ping {misses}/{MAX_MISSES}: {e:#}");
                    if misses >= MAX_MISSES {
                        sup.kill_hung(pid);
                        misses = 0;
                    }
                }
            }
        }
    })?;
    Ok(())
}
