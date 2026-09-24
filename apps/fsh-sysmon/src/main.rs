//! `fsh-sysmon`: a Ferroshell plugin publishing CPU and memory usage once a second.
//! It doubles as the reference example for writing plugins with `fsh-plugin-sdk`.
//!
//! Published sources (read by widgets as `Shell.source("org.ferroshell.system-monitor/<name>", ...)`):
//! - `cpu`: total CPU usage, percent (0-100)
//! - `mem`: physical memory in use, percent
//! - `mem-text`: e.g. "7.9 / 15.7 GB"

#![windows_subsystem = "windows"]

use std::time::Duration;

use fsh_plugin_sdk::{Context, Instance, Level, Plugin};
use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::GetSystemTimes;

struct SysMon;

fn ticks(t: FILETIME) -> u64 {
    (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime)
}

/// (idle, kernel + user) in 100 ns units since boot.
fn cpu_times() -> Option<(u64, u64)> {
    let (mut idle, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
    // SAFETY: all three pointers are to live, writable FILETIMEs.
    unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)) }.ok()?;
    Some((ticks(idle), ticks(kernel) + ticks(user)))
}

fn memory() -> Option<(f64, f64)> {
    let mut m = MEMORYSTATUSEX { dwLength: size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    // SAFETY: `m` is initialised with its size as the API requires.
    unsafe { GlobalMemoryStatusEx(&mut m) }.ok()?;
    let gb = |b: u64| b as f64 / (1u64 << 30) as f64;
    Some((gb(m.ullTotalPhys - m.ullAvailPhys), gb(m.ullTotalPhys)))
}

impl Plugin for SysMon {
    fn initialize(&mut self, ctx: &Context, _instances: Vec<Instance>) {
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let mut last = cpu_times();
            loop {
                std::thread::sleep(Duration::from_secs(1));
                let now = cpu_times();
                if let (Some((i0, t0)), Some((i1, t1))) = (last, now) {
                    let total = t1.saturating_sub(t0);
                    let busy = total.saturating_sub(i1.saturating_sub(i0));
                    let pct = if total == 0 { 0.0 } else { busy as f64 * 100.0 / total as f64 };
                    ctx.publish("cpu", (pct * 10.0).round() / 10.0);
                }
                last = now;
                match memory() {
                    Some((used, total)) => {
                        ctx.publish("mem", (used * 1000.0 / total).round() / 10.0);
                        ctx.publish("mem-text", format!("{used:.1} / {total:.1} GB"));
                    }
                    None => ctx.log(Level::Warn, "GlobalMemoryStatusEx failed"),
                }
            }
        });
    }
}

fn main() -> std::io::Result<()> {
    fsh_plugin_sdk::run(SysMon)
}
