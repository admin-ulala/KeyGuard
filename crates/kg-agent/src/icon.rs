//! 托盘图标：从内嵌的 .ico 里挑最大的一帧，转成 HICON。
//!
//! 不走资源文件（.rc + windres）是为了构建链路少一环：`include_bytes!` 把整个 ico
//! 编进二进制，运行时解析 ICONDIR，把某一帧的 DIB 交给 `CreateIconFromResourceEx`。

use std::ffi::c_void;

use windows_sys::Win32::UI::WindowsAndMessaging::{CreateIconFromResourceEx, DestroyIcon, HICON};

static ICO: &[u8] = include_bytes!("../../../assets/keyguard.ico");

/// ICO 头：reserved(u16) type(u16) count(u16)，之后每 16 字节一条 ICONDIRENTRY。
struct Entry {
    width: u8,
    bytes: u32,
    offset: u32,
}

fn entries() -> Vec<Entry> {
    let mut out = Vec::new();
    if ICO.len() < 6 || u16::from_le_bytes([ICO[2], ICO[3]]) != 1 {
        return out;
    }
    let count = u16::from_le_bytes([ICO[4], ICO[5]]) as usize;
    for i in 0..count {
        let p = 6 + i * 16;
        if p + 16 > ICO.len() {
            break;
        }
        out.push(Entry {
            width: ICO[p],
            bytes: u32::from_le_bytes([ICO[p + 8], ICO[p + 9], ICO[p + 10], ICO[p + 11]]),
            offset: u32::from_le_bytes([ICO[p + 12], ICO[p + 13], ICO[p + 14], ICO[p + 15]]),
        });
    }
    out
}

/// 取最接近 `want` 尺寸的一帧，加载成 HICON。失败返回 null。
pub fn load(want: u32) -> HICON {
    let list = entries();
    let Some(e) = list
        .iter()
        .min_by_key(|e| (e.width as i32 - want as i32).abs())
    else {
        return std::ptr::null_mut();
    };
    let start = e.offset as usize;
    let end = start + e.bytes as usize;
    if end > ICO.len() {
        return std::ptr::null_mut();
    }
    let blob = &ICO[start..end];
    let size = if e.width == 0 { 256 } else { e.width as u32 };

    unsafe {
        CreateIconFromResourceEx(
            blob.as_ptr(),
            blob.len() as u32,
            1,          // fIcon
            0x0003_0000, // 版本 3.0
            size as i32,
            size as i32,
            0, // LR_DEFAULTCOLOR
        )
    }
}

#[allow(dead_code)] // 图标常驻进程生命周期，重载时才需要
pub fn destroy(icon: HICON) {
    if !icon.is_null() {
        unsafe {
            DestroyIcon(icon);
        }
    }
}

#[allow(dead_code)]
fn _assert_send(_: *mut c_void) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ico_parses_and_loads() {
        // 内嵌 ICO 的目录项必须能被我们自己的解析器读出来
        let es = entries();
        assert!(es.len() >= 3, "ICO 里应该有多档尺寸，实际 {}", es.len());
        assert!(es.iter().any(|e| e.width == 32), "缺 32px 档");

        // 真造一个 HICON：返回 null 就说明资源头或者位深参数不对
        for size in [16u32, 32, 48, 64] {
            let h = load(size);
            assert!(!h.is_null(), "{size}px 造 HICON 失败");
            destroy(h);
        }
    }
}
