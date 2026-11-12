//! Agent 侧的进程卫生：单实例 + 隐藏"自己独占"的控制台。
//!
//! 两件事都属于"看起来像流氓软件"的体验问题：
//! 1. 计划任务版和用户手动版同时跑 → 两个 Agent 各挂一条钩子，
//!    每次按键被吞两遍、计数翻倍、托盘出现两个图标。用命名互斥体挡掉后来者。
//! 2. 任务拉起的是控制台程序 → 登录后桌面上留一个黑窗，还会抢前台。
//!    只在"控制台是本进程独占"时才隐藏，避免把用户自己的终端一起藏了。

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS};
use windows_sys::Win32::System::Console::{GetConsoleProcessList, GetConsoleWindow, SetConsoleTitleW};
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetShellWindow, SetForegroundWindow, ShowWindow, SW_HIDE,
};

const MUTEX: &str = "Local\\KeyGuard.Agent.v1";
const TITLE: &str = "KeyGuard Agent";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 抢 Agent 单实例锁。false = 已经有一个在跑。
pub fn acquire() -> bool {
    // 测试/灰度：换了管道名的实例互不相干，不该被同一把锁挡住
    if std::env::var("KEYGUARD_PIPE").is_ok() {
        return true;
    }
    let name = wide(MUTEX);
    unsafe {
        let h = CreateMutexW(std::ptr::null_mut(), 1, name.as_ptr());
        if h.is_null() {
            return true;
        }
        if GetLastError() == ERROR_ALREADY_EXISTS {
            CloseHandle(h);
            return false;
        }
        true
    }
}

/// 隐藏控制台并把焦点还给桌面（只在 `--agent` 常驻模式下调用）。
///
/// 关键判断是 `GetConsoleProcessList`：如果控制台里除了我们还挂着别人
/// （比如用户是在自己的 PowerShell 里手动敲的 `kg-agent --agent`），
/// 那就不能藏 —— 藏了等于把用户终端弄没了。
pub fn hide_console() {
    unsafe {
        let mut pids = [0u32; 8];
        let n = GetConsoleProcessList(pids.as_mut_ptr(), pids.len() as u32);
        if n != 1 {
            return;
        }

        let title = wide(TITLE);
        SetConsoleTitleW(title.as_ptr());

        let hwnd = GetConsoleWindow();
        if hwnd.is_null() {
            return;
        }
        ShowWindow(hwnd, SW_HIDE);

        // 藏掉之后前台还是这个隐形窗口，登录后第一次按键会没有落点。
        // 把焦点交回桌面（explorer），等于"什么都没抢"。
        let shell = GetShellWindow();
        if !shell.is_null() {
            SetForegroundWindow(shell);
        }
    }
}
