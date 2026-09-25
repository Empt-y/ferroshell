//! File operations as Explorer does them (IFileOperation): progress and conflict dialogs,
//! the Recycle Bin, and Ctrl+Z in Explorer undoes them.

use std::path::Path;

use anyhow::Context;
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
use windows::Win32::UI::Shell::{FileOperation, IFileOperation, IShellItem, SHCreateItemFromParsingName};
use windows::core::PCWSTR;

use crate::{Hwnd, wide};

const FOF_ALLOWUNDO: u32 = 0x40;
const FOFX_RECYCLEONDELETE: u32 = 0x0008_0000;
const FOFX_ADDUNDORECORD: u32 = 0x2000_0000;

fn item(path: &Path) -> anyhow::Result<IShellItem> {
    let w = wide(&path.to_string_lossy());
    unsafe { SHCreateItemFromParsingName(PCWSTR(w.as_ptr()), None) }.with_context(|| path.display().to_string())
}

fn operation(owner: Option<Hwnd>, flags: u32) -> anyhow::Result<IFileOperation> {
    let op: IFileOperation = unsafe { CoCreateInstance(&FileOperation, None, CLSCTX_ALL) }.context("IFileOperation")?;
    unsafe {
        op.SetOperationFlags(windows::Win32::UI::Shell::FILEOPERATION_FLAGS(flags))?;
        if let Some(h) = owner {
            op.SetOwnerWindow(h.raw())?;
        }
    }
    Ok(op)
}

/// Deletes to the Recycle Bin, or permanently (Windows asks first) with `permanently`.
/// Blocks while Windows works (and shows progress); needs COM (STA).
pub fn delete(paths: &[&Path], permanently: bool, owner: Option<Hwnd>) -> anyhow::Result<()> {
    let flags = if permanently { FOF_ALLOWUNDO } else { FOF_ALLOWUNDO | FOFX_RECYCLEONDELETE | FOFX_ADDUNDORECORD };
    let op = operation(owner, flags)?;
    for p in paths {
        unsafe { op.DeleteItem(&item(p)?, None) }.with_context(|| p.display().to_string())?;
    }
    unsafe { op.PerformOperations() }.context("deleting")?;
    Ok(())
}

/// Renames a file or folder (`new_name` is a name, not a path).
pub fn rename(path: &Path, new_name: &str, owner: Option<Hwnd>) -> anyhow::Result<()> {
    let op = operation(owner, FOF_ALLOWUNDO | FOFX_ADDUNDORECORD)?;
    let name = wide(new_name);
    unsafe {
        op.RenameItem(&item(path)?, PCWSTR(name.as_ptr()), None)?;
        op.PerformOperations()
    }
    .with_context(|| format!("renaming {} to {new_name}", path.display()))
}
