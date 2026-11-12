//! 把 schtasks 的控制台输出（GBK）解成 Rust 字符串。
//!
//! 直接 `String::from_utf8_lossy` 会得到一堆问号——中文系统的控制台代码页是 936，
//! 必须走 `MultiByteToWideChar(CP_ACP)`。

use windows_sys::Win32::Globalization::{MultiByteToWideChar, CP_ACP};

pub fn console_text(bytes: &[u8]) -> Option<String> {
    let bytes = bytes.strip_suffix(b"\r\n").unwrap_or(bytes);
    if bytes.is_empty() {
        return None;
    }
    let n = unsafe {
        MultiByteToWideChar(
            CP_ACP,
            0,
            bytes.as_ptr(),
            bytes.len() as i32,
            std::ptr::null_mut(),
            0,
        )
    };
    if n <= 0 {
        return Some(String::from_utf8_lossy(bytes).trim().to_string());
    }
    let mut buf = vec![0u16; n as usize];
    let written = unsafe {
        MultiByteToWideChar(
            CP_ACP,
            0,
            bytes.as_ptr(),
            bytes.len() as i32,
            buf.as_mut_ptr(),
            n,
        )
    };
    buf.truncate(written.max(0) as usize);
    let s = String::from_utf16_lossy(&buf);
    let t = s.trim().to_string();
    if t.is_empty() {
        None
    } else {
        Some(t)
    }
}
