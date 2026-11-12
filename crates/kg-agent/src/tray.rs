//! 托盘图标 + 全局解除热键。
//!
//! 全部活在一个专用线程里：一个 `HWND_MESSAGE` 消息窗收 `WM_HOTKEY` 与托盘回调，
//! 消息循环卡在 `GetMessageW` 上，空闲时零 CPU。
//!
//! 为什么热键要单独 `RegisterHotKey` 而不是复用钩子里的 F8：
//! 钩子在「已解除」状态下是 pass-through 的，但 `Ctrl+Alt+Shift+F12` 必须在任何状态下
//! 都能把锁解掉，包括游戏卡在加载界面、钩子还没来得及装的时候。注册热键由系统在输入
//! 队列之前处理，不依赖钩子是否 armed。

use std::path::PathBuf;
use std::sync::atomic::{AtomicIsize, AtomicPtr, Ordering};
use std::sync::Arc;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::System::Diagnostics::Debug::MessageBeep;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::RegisterHotKey;
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DispatchMessageW,
    GetCursorPos, GetMessageW, MessageBoxW, PostMessageW, PostQuitMessage, RegisterClassW,
    SetForegroundWindow, SetTimer, TrackPopupMenu, TranslateMessage, MSG,
    TPM_BOTTOMALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_DESTROY, WM_HOTKEY, WM_LBUTTONUP, WM_NULL,
    WM_RBUTTONUP, WM_TIMER, WNDCLASSW,
};

use kg_core::state::{mode_name, Shared};

const TRAY_MSG: u32 = 0x8000 + 1; // WM_APP + 1
const HOTKEY_ID: i32 = 0xA1;
const TIMER_ID: usize = 1;

const MOD_ALT: u32 = 0x0001;
const MOD_CONTROL: u32 = 0x0002;
const MOD_SHIFT: u32 = 0x0004;
const MOD_NOREPEAT: u32 = 0x4000;
const VK_F12: u32 = 0x7B;

const MF_STRING: u32 = 0x0000;
const MF_SEPARATOR: u32 = 0x0800;
const MF_GRAYED: u32 = 0x0001;

const CMD_PANIC: usize = 1;
const CMD_OPEN: usize = 2;
const CMD_AUTOSTART: usize = 3;
const CMD_QUIT: usize = 4;

static STATE: AtomicPtr<Shared> = AtomicPtr::new(std::ptr::null_mut());
static UI_PATH: AtomicIsize = AtomicIsize::new(0);

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn ui_path() -> Option<String> {
    let p = UI_PATH.load(Ordering::Relaxed);
    if p == 0 {
        return None;
    }
    // SAFETY: 存进去的是 Box::into_raw 出来的 Vec<u16>，进程存活期间一直有效
    let v = unsafe { &*(p as *const Vec<u16>) };
    Some(String::from_utf16_lossy(&v[..v.len().saturating_sub(1)]))
}

fn shared<'a>() -> Option<&'a Shared> {
    let p = STATE.load(Ordering::Acquire);
    if p.is_null() {
        None
    } else {
        Some(unsafe { &*p })
    }
}

/// 「本次会话别再锁了」+ 立刻解锁。热键、托盘菜单、IPC 的 panic_release 共用。
pub fn release_now(st: &Shared) {
    st.set_hold(false);
    st.request_panic();
    st.set_mode(kg_core::state::MODE_TEMP_UNLOCK);
    unsafe {
        MessageBeep(0);
    }
}

/// 打开主界面。Agent 是 High，直接 `CreateProcess` 会让界面继承高完整性 ——
/// 交给 explorer（Medium）去拉，界面才会跑在 Medium。
pub fn open_ui() {
    if let Some(path) = ui_path() {
        let _ = std::process::Command::new("explorer.exe").arg(path).spawn();
    }
}

fn set_tip(nid: &mut NOTIFYICONDATAW, text: &str) {
    let w = wide(text);
    let n = w.len().min(nid.szTip.len());
    nid.szTip[..n].copy_from_slice(&w[..n]);
    nid.szTip[n - 1] = 0;
}

fn build_tip(mode: u8) -> String {
    format!("KeyGuard · {}", mode_name(mode))
}

fn notify_remove(hwnd: HWND) {
    let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = 1;
    unsafe {
        Shell_NotifyIconW(NIM_DELETE, &nid);
    }
}

fn refresh_tip(hwnd: HWND) {
    let Some(st) = shared() else { return };
    let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = 1;
    nid.uFlags = NIF_TIP;
    set_tip(&mut nid, &build_tip(st.current_mode()));
    unsafe {
        Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
}

unsafe fn toggle_autostart() {
    let on = crate::autostart::is_enabled();
    let r = if on {
        crate::autostart::disable()
    } else {
        crate::autostart::enable()
    };
    if let Err(e) = r {
        let text = wide(&format!("设置开机自启失败：{e}"));
        let cap = wide("KeyGuard");
        MessageBoxW(std::ptr::null_mut(), text.as_ptr(), cap.as_ptr(), 0x0000_0010);
    }
}

unsafe fn show_menu(hwnd: HWND) {
    let menu = CreatePopupMenu();
    if menu.is_null() {
        return;
    }
    let mode = shared().map(|s| s.current_mode()).unwrap_or(0);
    let auto = crate::autostart::is_enabled();

    let tip = match mode {
        1 => "状态：正在拦截",
        2 => "状态：钩子异常（功能降级）",
        _ => "状态：待命",
    };
    AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, wide(tip).as_ptr());
    AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());

    AppendMenuW(menu, MF_STRING, CMD_OPEN, wide("打开主界面…").as_ptr());
    // 没在拦截时「解除」是空操作，置灰比点了没反应好
    let panic_flag = if mode == 1 { MF_STRING } else { MF_STRING | MF_GRAYED };
    AppendMenuW(menu, panic_flag, CMD_PANIC, wide("立即解除本次拦截").as_ptr());

    AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
    AppendMenuW(
        menu,
        MF_STRING,
        CMD_AUTOSTART,
        wide(if auto { "开机自动运行：开" } else { "开机自动运行：关" }).as_ptr(),
    );

    AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
    AppendMenuW(menu, MF_STRING, CMD_QUIT, wide("退出 KeyGuard（停止拦截）").as_ptr());

    let mut pt = POINT { x: 0, y: 0 };
    GetCursorPos(&mut pt);
    // 不先抢前台的话，点空白处菜单不消失（Win32 的老毛病）
    SetForegroundWindow(hwnd);
    let cmd = TrackPopupMenu(
        menu,
        TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN,
        pt.x,
        pt.y,
        0,
        hwnd,
        std::ptr::null(),
    );
    PostMessageW(hwnd, WM_NULL, 0, 0);
    DestroyMenu(menu);

    match cmd as usize {
        CMD_PANIC => {
            if let Some(st) = shared() {
                release_now(st);
            }
            refresh_tip(hwnd);
        }
        CMD_OPEN => open_ui(),
        CMD_AUTOSTART => {
            toggle_autostart();
            refresh_tip(hwnd);
        }
        CMD_QUIT => {
            // 退出前把系统状态还回去：没人拦了就不该继续吞键
            for line in kg_core::win::restore_system_state() {
                println!("{line}");
            }
            if let Some(st) = shared() {
                st.set_blocking(false);
            }
            notify_remove(hwnd);
            PostQuitMessage(0);
            std::process::exit(0);
        }
        _ => {}
    }
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_HOTKEY if wparam as i32 == HOTKEY_ID => {
            if let Some(st) = shared() {
                release_now(st);
            }
            refresh_tip(hwnd);
            0
        }
        TRAY_MSG => {
            let ev = (lparam as u32) & 0xFFFF;
            match ev {
                WM_RBUTTONUP => show_menu(hwnd),
                WM_LBUTTONUP => open_ui(),
                _ => {}
            }
            0
        }
        WM_TIMER if wparam == TIMER_ID => {
            refresh_tip(hwnd);
            0
        }
        WM_DESTROY => {
            notify_remove(hwnd);
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// 起托盘线程。`ui` 是 kg-ui.exe 的绝对路径（调用方按自身位置算）。
pub fn spawn(st: Arc<Shared>, ui: Option<PathBuf>) -> std::io::Result<()> {
    if let Some(p) = ui {
        let v = wide(&p.to_string_lossy());
        UI_PATH.store(Box::into_raw(Box::new(v)) as isize, Ordering::Release);
    }
    STATE.store(Arc::into_raw(st) as *mut Shared, Ordering::Release);

    std::thread::Builder::new()
        .name("kg-tray".into())
        .spawn(|| unsafe { run() })?;
    Ok(())
}

unsafe fn run() {
    let hinst = GetModuleHandleW(std::ptr::null());
    let cls = wide("KeyGuardTrayWnd");
    let wc = WNDCLASSW {
        style: 0,
        lpfnWndProc: Some(wnd_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: hinst,
        hIcon: std::ptr::null_mut(),
        hCursor: std::ptr::null_mut(),
        hbrBackground: std::ptr::null_mut(),
        lpszMenuName: std::ptr::null(),
        lpszClassName: cls.as_ptr(),
    };
    RegisterClassW(&wc);

    // HWND_MESSAGE = -3：消息专用窗，不参与窗口枚举、不上任务栏
    let hwnd = CreateWindowExW(
        0,
        cls.as_ptr(),
        wide("KeyGuard").as_ptr(),
        0,
        0,
        0,
        0,
        0,
        (-3isize) as HWND,
        std::ptr::null_mut(),
        hinst,
        std::ptr::null_mut(),
    );
    if hwnd.is_null() {
        return;
    }

    let icon = crate::icon::load(32);

    let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = 1;
    nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    nid.uCallbackMessage = TRAY_MSG;
    nid.hIcon = icon;
    let mode = shared().map(|s| s.current_mode()).unwrap_or(0);
    set_tip(&mut nid, &build_tip(mode));
    let added = Shell_NotifyIconW(NIM_ADD, &nid) != 0;

    let hot = RegisterHotKey(
        hwnd,
        HOTKEY_ID,
        MOD_CONTROL | MOD_ALT | MOD_SHIFT | MOD_NOREPEAT,
        VK_F12,
    ) != 0;

    println!(
        "托盘: {} · 热键 Ctrl+Alt+Shift+F12: {}",
        if added { "已显示" } else { "添加失败" },
        if hot { "已注册" } else { "注册失败（可能被别的程序占用）" }
    );

    SetTimer(hwnd, TIMER_ID, 2000, None);

    let mut msg: MSG = std::mem::zeroed();
    while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
}
