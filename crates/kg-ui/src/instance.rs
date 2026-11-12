//! 单实例：命名互斥体 + 把已存在的窗口拉到前台。
//!
//! 托盘菜单「打开主界面」可能被连点，也可能用户自己又双击了一次 exe。
//! 没有这把锁的话会开出两个窗口、两份状态轮询，看起来像幽灵进程。

use std::ffi::c_void;

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HWND, LPARAM};
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowTextW, IsWindowVisible, SetForegroundWindow, ShowWindow, SW_RESTORE,
    SW_SHOW,
};

const MUTEX: &str = "Local\\KeyGuard.UI.v1";
const TITLE: &str = "KeyGuard";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 抢单实例锁。返回 true 表示本进程是唯一的那个。
pub fn acquire() -> bool {
    let name = wide(MUTEX);
    unsafe {
        let h = CreateMutexW(std::ptr::null_mut(), 1, name.as_ptr());
        if h.is_null() {
            return true; // 建不出来就当没锁，宁可开两个也别打不开
        }
        if GetLastError() == ERROR_ALREADY_EXISTS {
            CloseHandle(h);
            return false;
        }
        // 故意不关句柄：它要活到进程结束，句柄随进程回收
        true
    }
}

struct Finder {
    found: HWND,
    /// 第一轮只要看得见的；找不到再来一轮，把「收进托盘」藏起来的那个也算上。
    visible_only: bool,
}

unsafe extern "system" fn enum_cb(hwnd: HWND, lparam: LPARAM) -> windows_sys::Win32::Foundation::BOOL {
    let f = &mut *(lparam as *mut Finder);
    if f.visible_only && IsWindowVisible(hwnd) == 0 {
        return 1;
    }
    let mut buf = [0u16; 128];
    let n = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
    if n > 0 {
        let t = String::from_utf16_lossy(&buf[..n as usize]);
        if t == TITLE || t.starts_with("KeyGuard") {
            f.found = hwnd;
            return 0; // 找到就停
        }
    }
    1
}

/// 把已经在跑的那个窗口显示出来并置前。
pub fn focus_existing() -> bool {
    // 两轮：先找可见窗口（正常情况），再找被「收进托盘」藏起来的那个。
    // 少了第二轮，隐藏后的窗口谁都叫不回来 —— 托盘点一下只多出一个立刻退出的进程。
    for visible_only in [true, false] {
        let mut f = Finder {
            found: std::ptr::null_mut(),
            visible_only,
        };
        unsafe {
            EnumWindows(Some(enum_cb), &mut f as *mut Finder as LPARAM);
            if f.found.is_null() {
                continue;
            }
            // 可见的按「还原」亮出来，藏起来的用 SW_SHOW 直接显形
            ShowWindow(f.found, if visible_only { SW_RESTORE } else { SW_SHOW });
            SetForegroundWindow(f.found);
        }
        return true;
    }
    false
}

#[allow(dead_code)]
fn _unused(_: *mut c_void) {}
