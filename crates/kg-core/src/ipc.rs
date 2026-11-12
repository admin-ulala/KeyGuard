//! 命名管道：Agent（High）做服务端，界面（Medium）做客户端。
//!
//! 安全描述符 \`D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;<user-sid>)S:(ML;;NW;;;ME)\`：
//! - `SY` 系统、`BA` 管理员、`<user-sid>` 当前用户 —— 其余人全拒
//! - `S:(ML;;NW;;;ME)` 是关键：不写这条，标了 High 的管道会挡住 Medium 进程，
//!   而 `CreateFile` 只会返回一句话都不说的 Access Denied
//!
//! 消息模式（PIPE_TYPE_MESSAGE）：一条 WriteFile = 一条消息 = 一个 JSON，不用自己做分帧。

use std::ffi::c_void;
use std::sync::Arc;

use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, LocalFree, ERROR_MORE_DATA, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED,
    HANDLE, HLOCAL,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, TokenUser, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, ReadFile, WriteFile, FILE_GENERIC_READ, FILE_GENERIC_WRITE, OPEN_EXISTING,
    PIPE_ACCESS_DUPLEX,
};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PeekNamedPipe, PIPE_READMODE_MESSAGE,
    PIPE_TYPE_MESSAGE, PIPE_WAIT, WaitNamedPipeW,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

use crate::proto::{Request, Response};

pub const PIPE_NAME: &str = r"\\.\pipe\keyguard.v1";

/// 管道名。默认写死；设 `KEYGUARD_PIPE` 可换一条，让新旧两版并行跑（灰度与联调都靠它）。
pub fn pipe_name() -> String {
    match std::env::var("KEYGUARD_PIPE") {
        Ok(v) if !v.trim().is_empty() => {
            let v = v.trim().to_string();
            if v.starts_with(r"\\.\pipe\") {
                v
            } else {
                format!(r"\\.\pipe\{v}")
            }
        }
        _ => PIPE_NAME.to_string(),
    }
}

const BUF: usize = 64 * 1024;
const MAX_CLIENTS: u32 = 4;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 当前用户 SID 的字符串形式（`S-1-5-21-...`）。
pub fn current_user_sid() -> Option<String> {
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;

    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return None;
        }
        let mut buf = [0u64; 64]; // 256 字节，TOKEN_USER 足够
        let mut len: u32 = 0;
        let ok = GetTokenInformation(
            token,
            TokenUser,
            buf.as_mut_ptr() as *mut c_void,
            std::mem::size_of_val(&buf) as u32,
            &mut len,
        );
        CloseHandle(token);
        if ok == 0 {
            return None;
        }
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut s: *mut u16 = std::ptr::null_mut();
        if ConvertSidToStringSidW(user.User.Sid, &mut s) == 0 {
            return None;
        }
        let mut n = 0usize;
        while *s.add(n) != 0 {
            n += 1;
        }
        let out = String::from_utf16_lossy(std::slice::from_raw_parts(s, n));
        LocalFree(s as HLOCAL);
        Some(out)
    }
}

/// 生成只允许「系统 / 管理员 / 当前用户」访问、且不挡 Medium 请求者的 SDDL。
pub fn sddl_for_current_user() -> String {
    let sid = current_user_sid().unwrap_or_else(|| "BA".to_string());
    format!("D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;{sid})S:(ML;;NW;;;ME)")
}

/// 把 SDDL 变成 SECURITY_ATTRIBUTES。返回的指针要一直活到 CreateNamedPipe 之后。
struct Sd(*mut c_void);

impl Sd {
    fn from_sddl(sddl: &str) -> Option<(Self, SECURITY_ATTRIBUTES)> {
        let w = wide(sddl);
        let mut psd: *mut c_void = std::ptr::null_mut();
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                w.as_ptr(),
                SDDL_REVISION_1,
                &mut psd,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 || psd.is_null() {
            return None;
        }
        let sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: psd,
            bInheritHandle: 0,
        };
        Some((Sd(psd), sa))
    }
}

impl Drop for Sd {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { LocalFree(self.0 as HLOCAL) };
        }
    }
}

pub type Handler = Arc<dyn Fn(Request) -> Response + Send + Sync>;

/// 起服务端。阻塞在 accept 循环里，调用方一般直接扔进线程。
pub fn serve(handler: Handler) -> Result<(), u32> {
    let sddl = sddl_for_current_user();
    let name = wide(&pipe_name());

    loop {
        let Some((_sd, sa)) = Sd::from_sddl(&sddl) else {
            return Err(unsafe { GetLastError() });
        };

        let pipe = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT,
                MAX_CLIENTS,
                BUF as u32,
                BUF as u32,
                0,
                &sa,
            )
        };
        if pipe.is_null() {
            return Err(unsafe { GetLastError() });
        }

        // 等一个客户端；已连上（ERROR_PIPE_CONNECTED）也算成功
        let connected = unsafe { ConnectNamedPipe(pipe, std::ptr::null_mut()) } != 0
            || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;

        if !connected {
            unsafe {
                CloseHandle(pipe);
            }
            continue;
        }

        let handler = handler.clone();
        // HANDLE 是 *mut c_void，不满足 Send；这里转成整数带进线程再转回来。
        // 所有权是独占的：CreateNamedPipeW 生成的实例只被这一个线程用。
        let raw = pipe as usize;
        std::thread::spawn(move || {
            let pipe = raw as HANDLE;
            handle_client(pipe, handler);
            unsafe {
                DisconnectNamedPipe(pipe);
                CloseHandle(pipe);
            }
        });
    }
}

fn handle_client(pipe: HANDLE, handler: Handler) {
    let mut buf = vec![0u8; BUF];
    loop {
        let mut read: u32 = 0;
        let ok = unsafe {
            ReadFile(
                pipe,
                buf.as_mut_ptr(),
                buf.len() as u32,
                &mut read,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            let e = unsafe { GetLastError() };
            if e == ERROR_MORE_DATA {
                let resp = Response::err("消息过大（上限 64 KB）");
                write_msg(pipe, &resp);
            }
            return; // 客户端断开
        }
        if read == 0 {
            return;
        }

        let text = String::from_utf8_lossy(&buf[..read as usize]);
        let resp = match serde_json::from_str::<Request>(text.trim()) {
            Ok(req) => handler(req),
            Err(e) => Response::err(format!("无法解析请求: {e}")),
        };
        if !write_msg(pipe, &resp) {
            return;
        }
    }
}

fn write_msg(pipe: HANDLE, resp: &Response) -> bool {
    let Ok(mut body) = serde_json::to_vec(resp) else {
        return false;
    };
    body.push(b'\n');
    let mut written: u32 = 0;
    let ok = unsafe {
        WriteFile(
            pipe,
            body.as_ptr(),
            body.len() as u32,
            &mut written,
            std::ptr::null_mut(),
        )
    };
    ok != 0
}

/// 客户端：一次请求一次响应。超时 2 秒（Agent 卡死时界面不能跟着卡死）。
pub fn call(req: &Request) -> Result<Response, String> {
    let name = wide(&pipe_name());
    let mut handle = unsafe {
        CreateFileW(
            name.as_ptr(),
            FILE_GENERIC_READ | FILE_GENERIC_WRITE,
            0,
            std::ptr::null_mut(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        )
    };
    if handle.is_null() {
        let e = unsafe { GetLastError() };
        if e == ERROR_PIPE_BUSY {
            if unsafe { WaitNamedPipeW(name.as_ptr(), 2000) } == 0 {
                return Err("Agent 忙，等待超时".into());
            }
            handle = unsafe {
                CreateFileW(
                    name.as_ptr(),
                    FILE_GENERIC_READ | FILE_GENERIC_WRITE,
                    0,
                    std::ptr::null_mut(),
                    OPEN_EXISTING,
                    0,
                    std::ptr::null_mut(),
                )
            };
        }
        if handle.is_null() {
            return Err(match e {
                2 => "Agent 没有在运行（管道不存在）".to_string(),
                5 => "管道拒绝访问：Agent 的完整性级别高于本进程，且 SDDL 里没有 S:(ML;;NW;;;ME)".to_string(),
                _ => format!("无法连接 Agent（Win32 {e}）"),
            });
        }
    }

    let mut body = serde_json::to_vec(req).map_err(|e| e.to_string())?;
    body.push(b'\n');
    let mut written: u32 = 0;
    let ok = unsafe {
        WriteFile(
            handle,
            body.as_ptr(),
            body.len() as u32,
            &mut written,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        unsafe { CloseHandle(handle) };
        return Err("写入请求失败".into());
    }

    let mut buf = vec![0u8; BUF];
    let mut read: u32 = 0;
    // ReadFile 在没有数据时会一直阻塞；界面每 800ms 在 UI 线程上同步调一次，
    // Agent 一旦卡住界面就整片白屏。先 Peek 轮询，最多等 2 秒，超时就把这条管道丢掉。
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(2000);
    loop {
        let mut avail: u32 = 0;
        let peek = unsafe {
            PeekNamedPipe(
                handle,
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                &mut avail,
                std::ptr::null_mut(),
            )
        };
        if peek == 0 {
            unsafe { CloseHandle(handle) };
            return Err("Agent 断开了连接".into());
        }
        if avail > 0 {
            break;
        }
        if std::time::Instant::now() >= deadline {
            unsafe { CloseHandle(handle) };
            return Err("Agent 2 秒没有回话（可能卡住了）".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let ok = unsafe {
        ReadFile(
            handle,
            buf.as_mut_ptr(),
            buf.len() as u32,
            &mut read,
            std::ptr::null_mut(),
        )
    };
    unsafe { CloseHandle(handle) };
    if ok == 0 {
        return Err("读取响应失败".into());
    }
    serde_json::from_slice(&buf[..read as usize]).map_err(|e| format!("响应无法解析: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sddl_has_medium_label() {
        let s = sddl_for_current_user();
        assert!(s.starts_with("D:P(A;;GA;;;SY)"), "SID 应在最前面拿到系统");
        assert!(s.contains("S:(ML;;NW;;;ME)"), "缺了 ML 标签，Medium 界面连不上 High Agent");
    }

    #[test]
    fn user_sid_looks_like_sid() {
        if let Some(sid) = current_user_sid() {
            assert!(sid.starts_with("S-1-5-21-") || sid.starts_with("S-1-12-"), "拿到的是 {sid}");
        }
    }
}
