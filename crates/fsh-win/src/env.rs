//! The user's environment as Windows builds it from the registry, and updating this
//! process's environment (which the apps we start inherit).

use anyhow::Context;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Security::{TOKEN_DUPLICATE, TOKEN_IMPERSONATE, TOKEN_QUERY};
use windows::Win32::System::Environment::{CreateEnvironmentBlock, DestroyEnvironmentBlock, SetEnvironmentVariableW};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::core::PCWSTR;

use crate::wide;

/// Parses a `NAME=value\0NAME=value\0\0` UTF-16 block. Names may start with `=` (the
/// per-drive current directories), so split at the first `=` after the first character.
fn parse_block(units: &[u16]) -> Vec<(String, String)> {
    units
        .split(|&u| u == 0)
        .take_while(|s| !s.is_empty())
        .filter_map(|s| {
            let s = String::from_utf16_lossy(s);
            let split = s.char_indices().skip(1).find(|(_, c)| *c == '=')?.0;
            Some((s[..split].to_owned(), s[split + 1..].to_owned()))
        })
        .collect()
}

/// The user environment as a newly started Explorer would get it: system and user
/// variables from the registry, expanded, without anything inherited by this process.
pub fn user_environment() -> anyhow::Result<Vec<(String, String)>> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY | TOKEN_DUPLICATE | TOKEN_IMPERSONATE, &mut token)
            .context("OpenProcessToken")?;
        let mut block = std::ptr::null_mut();
        let made = CreateEnvironmentBlock(&mut block, Some(token), false);
        let _ = CloseHandle(token);
        made.context("CreateEnvironmentBlock")?;
        // Find the double NUL that ends the block.
        let p = block as *const u16;
        let mut len = 0usize;
        while !(*p.add(len) == 0 && *p.add(len + 1) == 0) {
            len += 1;
        }
        let vars = parse_block(std::slice::from_raw_parts(p, len + 2));
        let _ = DestroyEnvironmentBlock(block);
        Ok(vars)
    }
}

/// Sets and removes variables in this process's environment.
pub fn apply(set: &[(String, String)], remove: &[String]) {
    for (k, v) in set {
        let (k, v) = (wide(k), wide(v));
        unsafe {
            let _ = SetEnvironmentVariableW(PCWSTR(k.as_ptr()), PCWSTR(v.as_ptr()));
        }
    }
    for k in remove {
        let k = wide(k);
        unsafe {
            let _ = SetEnvironmentVariableW(PCWSTR(k.as_ptr()), PCWSTR::null());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_blocks() {
        let block: Vec<u16> = "=C:=C:\\x\0Path=a;b\0EMPTY=\0\0".encode_utf16().collect();
        assert_eq!(
            parse_block(&block),
            vec![
                ("=C:".to_string(), "C:\\x".to_string()),
                ("Path".to_string(), "a;b".to_string()),
                ("EMPTY".to_string(), String::new()),
            ]
        );
    }

    #[test]
    fn reads_the_user_environment() {
        let env = user_environment().unwrap();
        assert!(env.iter().any(|(k, _)| k.eq_ignore_ascii_case("Path")));
        assert!(env.iter().any(|(k, _)| k.eq_ignore_ascii_case("USERPROFILE")));
    }
}
