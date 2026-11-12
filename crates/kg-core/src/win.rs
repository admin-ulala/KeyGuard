//! Win32 查询封装。
//!
//! 这里只放「读」操作：读前台窗口、读进程、读令牌、读通知状态。
//! 任何一次调用失败都返回 `None`/保守值，绝不 panic —— 这些代码跑在自检线程里，
//! 一次异常就会让整个 Agent 哑掉。
//!
//! HWND 在 windows-sys 里是裸指针，但这些函数只做「检查空句柄 → 交给系统 API」，
//! 不会解引用调用方给的任意地址；clippy 的 not_unsafe_ptr_arg_deref 对不透明句柄会误报。
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use std::ffi::c_void;
use std::sync::OnceLock;

use windows_sys::Win32::Foundation::{CloseHandle, HWND, LPARAM, RECT};
use windows_sys::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, TokenElevation, TokenIntegrityLevel, TOKEN_ELEVATION, TOKEN_MANDATORY_LABEL,
    TOKEN_QUERY,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, OpenProcess, OpenProcessToken, QueryFullProcessImageNameW,
    PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::Shell::SHQueryUserNotificationState;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetForegroundWindow, GetWindow, GetWindowRect, GetWindowTextW,
    GetWindowThreadProcessId, IsWindowVisible, SystemParametersInfoW, GW_OWNER,
};

/// 当前进程是否已提权（High 完整性）。
///
/// 决定能不能拦「以管理员身份运行」的游戏：UIPI 之下，Medium 进程装的全局钩子
/// 收不到发往 High 窗口的按键，而且**一条日志都不会有**，所以必须主动自检。
pub fn is_elevated() -> bool {
    unsafe {
        let mut token: *mut c_void = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut elev = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut ret_len: u32 = 0;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            &mut elev as *mut _ as *mut c_void,
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut ret_len,
        );
        CloseHandle(token);
        ok != 0 && elev.TokenIsElevated != 0
    }
}

pub fn self_pid() -> u32 {
    unsafe { GetCurrentProcessId() }
}

/// 读进程的完整性级别 RID（Medium=0x2000，High=0x3000，System=0x4000）。
///
/// 打不开进程（提权游戏、受保护进程）一律返回 `None` —— 调用方要把它当成
/// 「比我们高」，这是唯一安全的默认值。
pub fn process_rid(pid: u32) -> Option<u32> {
    if pid == 0 {
        return None;
    }
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut token: *mut c_void = std::ptr::null_mut();
        if OpenProcessToken(h, TOKEN_QUERY, &mut token) == 0 {
            CloseHandle(h);
            return None;
        }

        // 8 字节对齐的缓冲：TOKEN_MANDATORY_LABEL 里有指针，不能随便找个 u8 数组
        let mut buf = [0u64; 32]; // 256 字节，足够放下一个 SID
        let mut ret_len: u32 = 0;
        let ok = GetTokenInformation(
            token,
            TokenIntegrityLevel,
            buf.as_mut_ptr() as *mut c_void,
            std::mem::size_of_val(&buf) as u32,
            &mut ret_len,
        );
        CloseHandle(token);
        CloseHandle(h);
        if ok == 0 {
            return None;
        }

        // 直接读 SID 二进制，不走 ConvertSidToStringSid（默认绑 A 版会出现字节颠倒的乱码）
        let label = &*(buf.as_ptr() as *const TOKEN_MANDATORY_LABEL);
        let sid = label.Label.Sid as *const u8;
        if sid.is_null() {
            return None;
        }
        let count = *sid.add(1) as usize;
        if count == 0 {
            return None;
        }
        let last = sid.add(8 + (count - 1) * 4) as *const u32;
        Some(*last)
    }
}

pub fn self_rid() -> Option<u32> {
    process_rid(self_pid())
}

/// 进程映像是 exe 文件名（不含路径）。打不开就返回 None（多半是权限更高的进程）。
pub fn process_exe(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut buf = [0u16; 512];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len);
        CloseHandle(h);
        if ok == 0 {
            return None;
        }
        let full = String::from_utf16_lossy(&buf[..len as usize]);
        Some(match full.rsplit_once('\\') {
            Some((_, name)) => name.to_string(),
            None => full,
        })
    }
}

pub fn foreground_window() -> HWND {
    unsafe { GetForegroundWindow() }
}

pub fn window_pid(h: HWND) -> u32 {
    if h.is_null() {
        return 0;
    }
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(h, &mut pid);
        pid
    }
}

pub fn window_class(h: HWND) -> String {
    if h.is_null() {
        return String::new();
    }
    unsafe {
        let mut buf = [0u16; 256];
        let n = GetClassNameW(h, buf.as_mut_ptr(), buf.len() as i32);
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }
}

pub fn window_title(h: HWND) -> String {
    if h.is_null() {
        return String::new();
    }
    unsafe {
        let mut buf = [0u16; 256];
        let n = GetWindowTextW(h, buf.as_mut_ptr(), buf.len() as i32);
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }
}

/// DWM 标记：cloaked 的窗口「存在但不可见」（虚拟桌面切换、UWP 挂起）。
/// 不排除它会把「已经切走的全屏窗口」当成前台，导致永远不解锁。
pub fn is_cloaked(h: HWND) -> bool {
    if h.is_null() {
        return false;
    }
    unsafe {
        let mut cloaked: i32 = 0;
        let hr = DwmGetWindowAttribute(
            h,
            DWMWA_CLOAKED as u32,
            &mut cloaked as *mut i32 as *mut c_void,
            std::mem::size_of::<i32>() as u32,
        );
        hr == 0 && cloaked != 0
    }
}

pub fn window_rect(h: HWND) -> Option<RECT> {
    if h.is_null() {
        return None;
    }
    unsafe {
        let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        if GetWindowRect(h, &mut r) == 0 {
            return None;
        }
        Some(r)
    }
}

pub fn monitor_rect(h: HWND) -> Option<RECT> {
    if h.is_null() {
        return None;
    }
    unsafe {
        let mon = MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST);
        if mon.is_null() {
            return None;
        }
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(mon, &mut mi) == 0 {
            return None;
        }
        Some(mi.rcMonitor)
    }
}

/// `SHQueryUserNotificationState` 的返回值：
/// 2=BUSY 3=RUNNING_D3D_FULL_SCREEN 4=PRESENTATION_MODE 5=ACCEPTS_NOTIFICATIONS
/// 6=QUIET_TIME 7=APP。取不到时返回 5（当作正常桌面，宁可不解锁也不要误锁）。
pub fn notification_state() -> i32 {
    unsafe {
        let mut q: i32 = 5;
        let hr = SHQueryUserNotificationState(&mut q);
        if hr < 0 {
            5
        } else {
            q
        }
    }
}

/// 窗口矩形是否盖住了所在显示器（允许 2px 误差，无边框全屏常有 1px 偏移）。
pub fn covers_monitor(h: HWND) -> bool {
    let (Some(w), Some(m)) = (window_rect(h), monitor_rect(h)) else {
        return false;
    };
    const TOL: i32 = 2;
    w.left <= m.left + TOL
        && w.top <= m.top + TOL
        && w.right >= m.right - TOL
        && w.bottom >= m.bottom - TOL
}

// ---- 计时 ----
// 只在启用诊断时采样，用作 p50/p99。QPC 两次调用约 30ns，相对 0.2ms 预算可忽略。

use windows_sys::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};

static QPC_FREQ: OnceLock<i64> = OnceLock::new();

/// QPC 频率（每秒计数）。只查一次，失败按 10MHz 兜底。
pub fn qpc_freq() -> i64 {
    *QPC_FREQ.get_or_init(|| unsafe {
        let mut f: i64 = 0;
        if QueryPerformanceFrequency(&mut f) == 0 || f <= 0 {
            10_000_000
        } else {
            f
        }
    })
}

#[inline]
pub fn ticks() -> i64 {
    unsafe {
        let mut t: i64 = 0;
        QueryPerformanceCounter(&mut t);
        t
    }
}

/// 从 t0 到现在的微秒数（向下取整）。
#[inline]
pub fn ticks_to_us(t0: i64) -> u32 {
    let d = ticks() - t0;
    if d <= 0 {
        return 0;
    }
    let us = (d as i128 * 1_000_000 / qpc_freq() as i128) as u64;
    us.min(u32::MAX as u64) as u32
}

// ---------------- 无障碍快捷键（SPI_*） ----------------

const SPIF_SENDCHANGE: u32 = 0x02;
const SPIF_UPDATEINIFILE: u32 = 0x01;
/// 落盘 + 广播。只广播不落盘的话，注销一次改动就没了。
const SPIF_PERSIST: u32 = SPIF_UPDATEINIFILE | SPIF_SENDCHANGE;

const SPI_GETSTICKYKEYS: u32 = 0x003A;
const SPI_SETSTICKYKEYS: u32 = 0x003B;
const SPI_GETFILTERKEYS: u32 = 0x0032;
const SPI_SETFILTERKEYS: u32 = 0x0033;
const SPI_GETTOGGLEKEYS: u32 = 0x0034;
const SPI_SETTOGGLEKEYS: u32 = 0x0035;
const SPI_GETHIGHCONTRAST: u32 = 0x0042;
const SPI_SETHIGHCONTRAST: u32 = 0x0043;

/// 高对比度当前是否真的开着。开着就别去写它的结构体，系统会把配色方案重放一遍。
const HCF_HIGHCONTRASTON: u32 = 0x1;

/// 三个无障碍结构里 `HOTKEYACTIVE` 都是 bit 2，只有高对比度叫 `HCF_HOTKEYACTIVE` 位置一样。
const HOTKEY_ACTIVE: u32 = 0x4;

#[repr(C)]
struct StickyKeys {
    cb_size: u32,
    flags: u32,
}

#[repr(C)]
struct FilterKeys {
    cb_size: u32,
    flags: u32,
    wait_ms: u32,
    delay_ms: u32,
    repeat_ms: u32,
    bounce_ms: u32,
}

#[repr(C)]
struct ToggleKeys {
    cb_size: u32,
    flags: u32,
}

#[repr(C)]
struct HighContrast {
    cb_size: u32,
    flags: u32,
    default_scheme: *mut u16,
}

/// 开关一条无障碍快捷键的"热键响应"，只翻转 HOTKEYACTIVE 那一位。
///
/// 两条硬规矩，都是被用户现场事故逼出来的：
///
/// 1. **读现状再改一位**。这些结构里还存着用户自己的粘滞键设置（三键关、声音提示、
///    可用的按键等），早先整体重写成 `flags = 0` 等于顺手把「粘滞键本身」也关了。
/// 2. **没变化就一个字节都不写**。每次 write 都会广播 WM_SETTINGCHANGE，explorer
///    收到就重读主题——用户会看到壁纸被换回主题自带的那张。以前每点一次开关广播四次。
pub fn set_accessibility_hotkey(id: &str, block: bool) -> bool {
    unsafe {
        match id {
            "sticky-keys" => {
                let mut s = StickyKeys {
                    cb_size: std::mem::size_of::<StickyKeys>() as u32,
                    flags: 0,
                };
                let ok = spi_get(SPI_GETSTICKYKEYS, &mut s as *mut _ as *mut core::ffi::c_void, s.cb_size);
                dbg(&format!("sticky get ok={ok} flags={:#x}", s.flags));
                if !ok {
                    return false;
                }
                let want = flip(s.flags, block);
                if want == s.flags {
                    return true;
                }
                s.flags = want;
                dbg(&format!("sticky set flags={want:#x}"));
                spi(SPI_SETSTICKYKEYS, &mut s as *mut _ as *mut core::ffi::c_void, s.cb_size)
            }
            "toggle-keys" => {
                let mut s = ToggleKeys {
                    cb_size: std::mem::size_of::<ToggleKeys>() as u32,
                    flags: 0,
                };
                let ok = spi_get(SPI_GETTOGGLEKEYS, &mut s as *mut _ as *mut core::ffi::c_void, s.cb_size);
                dbg(&format!("toggle get ok={ok} flags={:#x}", s.flags));
                if !ok {
                    return false;
                }
                let want = flip(s.flags, block);
                if want == s.flags {
                    return true;
                }
                s.flags = want;
                spi(SPI_SETTOGGLEKEYS, &mut s as *mut _ as *mut core::ffi::c_void, s.cb_size)
            }
            "filter-keys" => {
                let mut s = FilterKeys {
                    cb_size: std::mem::size_of::<FilterKeys>() as u32,
                    flags: 0,
                    wait_ms: 0,
                    delay_ms: 0,
                    repeat_ms: 0,
                    bounce_ms: 0,
                };
                let ok = spi_get(SPI_GETFILTERKEYS, &mut s as *mut _ as *mut core::ffi::c_void, s.cb_size);
                dbg(&format!("filter get ok={ok} flags={:#x}", s.flags));
                if !ok {
                    return false;
                }
                let want = flip(s.flags, block);
                if want == s.flags {
                    return true;
                }
                s.flags = want;
                spi(SPI_SETFILTERKEYS, &mut s as *mut _ as *mut core::ffi::c_void, s.cb_size)
            }
            "high-contrast" => {
                let mut s = HighContrast {
                    cb_size: std::mem::size_of::<HighContrast>() as u32,
                    flags: 0,
                    default_scheme: std::ptr::null_mut(),
                };
                let ok = spi_get(SPI_GETHIGHCONTRAST, &mut s as *mut _ as *mut core::ffi::c_void, s.cb_size);
                dbg(&format!("hc get ok={ok} flags={:#x}", s.flags));
                if !ok {
                    return false;
                }
                // 真的开着高对比度：不碰。写回这个结构体会让系统重放配色方案/主题。
                if s.flags & HCF_HIGHCONTRASTON != 0 {
                    return false;
                }
                let want = flip(s.flags, block);
                if want == s.flags {
                    return true;
                }
                s.flags = want;
                s.default_scheme = std::ptr::null_mut();
                spi(SPI_SETHIGHCONTRAST, &mut s as *mut _ as *mut core::ffi::c_void, s.cb_size)
            }
            _ => false,
        }
    }
}

/// block=true 表示要拦：清掉 HOTKEY_ACTIVE；false 表示放行：置上。
fn dbg(msg: &str) {
    if std::env::var("KEYGUARD_DEBUG_SPI").is_ok() {
        eprintln!("[spi] {msg}");
    }
}

fn flip(flags: u32, block: bool) -> u32 {
    if block {
        flags & !HOTKEY_ACTIVE
    } else {
        flags | HOTKEY_ACTIVE
    }
}

unsafe fn spi(action: u32, pv: *mut core::ffi::c_void, cb: u32) -> bool {
    SystemParametersInfoW(action, cb, pv, SPIF_PERSIST) != 0
}

unsafe fn spi_get(action: u32, pv: *mut core::ffi::c_void, cb: u32) -> bool {
    SystemParametersInfoW(action, cb, pv, 0) != 0
}

// ---------------- L2：扫描码映射 ----------------

/// `HKLM\SYSTEM\CurrentControlSet\Control\Keyboard Layout` 下的 `Scancode Map`。
///
/// 这是键盘驱动层的一张重映射表：把 Win 键（扫描码 `E05B` / `E05C`）映射到「无」，
/// 系统在驱动层就把这两个键吃掉，用户态没有任何进程参与 —— 反作弊看不到任何东西。
/// 代价是**只能处理单键**（拦不住 Alt+Tab，那需要组合键），而且必须重启才生效。
///
/// 二进制布局（标准 24 字节）：
/// ```text
/// 00000000 00000000  两个保留 DWORD
/// 03000000           映射条数 = 2 条映射 + 1 条终止符
/// 00005BE0           把 E05B（左 Win）映射到 0000（无键）
/// 00005CE0           把 E05C（右 Win）映射到 0000
/// 00000000           终止符
/// ```
const SCANCODE_KEY: &str = "SYSTEM\\CurrentControlSet\\Control\\Keyboard Layout";
const SCANCODE_VAL: &str = "Scancode Map";

const DISABLE_WIN_KEYS: [u8; 24] = [
    0x00, 0x00, 0x00, 0x00, //
    0x00, 0x00, 0x00, 0x00, //
    0x03, 0x00, 0x00, 0x00, // 条数
    0x00, 0x00, 0x5B, 0xE0, // 左 Win → 无
    0x00, 0x00, 0x5C, 0xE0, // 右 Win → 无
    0x00, 0x00, 0x00, 0x00, // 终止
];

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegSetValueExW, HKEY, HKEY_LOCAL_MACHINE,
    KEY_SET_VALUE, REG_BINARY,
};

/// 备份文件：记录「我们接管之前」这张表长什么样，用于精确还原。
///
/// - 文件不存在：这张表不归我们管，`disable = false` 时一个字都不动
/// - 内容 `absent`：接管时本来没有这个值，还原时删掉
/// - 十六进制字节：接管时用户/别的软件已经有一张表，还原时原样写回
///
/// 没有这份记录就盲删，会把用户自己配的键位映射（改键、跛脚键）一起干掉。
fn scancode_backup_path() -> std::path::PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("KeyGuard").join("scancode.bak")
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02X}"));
    }
    s
}

fn hex_decode(text: &str) -> Option<Vec<u8>> {
    let t: Vec<u8> = text.bytes().filter(|c| !c.is_ascii_whitespace()).collect();
    if t.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(t.len() / 2);
    for pair in t.chunks(2) {
        let hi = (pair[0] as char).to_digit(16)?;
        let lo = (pair[1] as char).to_digit(16)?;
        out.push((hi * 16 + lo) as u8);
    }
    Some(out)
}

/// 读当前 `Scancode Map` 的原始字节，`None` = 该值不存在。
fn scancode_map_raw() -> Option<Vec<u8>> {
    use windows_sys::Win32::System::Registry::{RegQueryValueExW, KEY_QUERY_VALUE};
    let sub = wide(SCANCODE_KEY);
    let name = wide(SCANCODE_VAL);
    let mut key: HKEY = std::ptr::null_mut();
    unsafe {
        if RegOpenKeyExW(HKEY_LOCAL_MACHINE, sub.as_ptr(), 0, KEY_QUERY_VALUE, &mut key) != 0 {
            return None;
        }
        let mut len = 0u32;
        let mut rc = RegQueryValueExW(
            key,
            name.as_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut len,
        );
        if rc != 0 || len == 0 {
            RegCloseKey(key);
            return None;
        }
        let mut buf = vec![0u8; len as usize];
        rc = RegQueryValueExW(
            key,
            name.as_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            buf.as_mut_ptr(),
            &mut len,
        );
        RegCloseKey(key);
        if rc != 0 {
            return None;
        }
        buf.truncate(len as usize);
        Some(buf)
    }
}

/// 底层写入：`Some(bytes)` 写值，`None` 删值。
fn write_scancode_raw(bytes: Option<&[u8]>) -> Result<(), String> {
    let sub = wide(SCANCODE_KEY);
    let name = wide(SCANCODE_VAL);
    let mut key: HKEY = std::ptr::null_mut();
    unsafe {
        let rc = RegOpenKeyExW(HKEY_LOCAL_MACHINE, sub.as_ptr(), 0, KEY_SET_VALUE, &mut key);
        if rc != 0 {
            return Err(format!(
                "打不开 HKLM\\{}（错误 {rc}）：需要管理员权限",
                SCANCODE_KEY
            ));
        }
        let out = match bytes {
            Some(b) => RegSetValueExW(key, name.as_ptr(), 0, REG_BINARY, b.as_ptr(), b.len() as u32),
            None => {
                let rc = RegDeleteValueW(key, name.as_ptr());
                // 本来就没有这个值也算成功
                if rc == 2 {
                    0
                } else {
                    rc
                }
            }
        };
        RegCloseKey(key);
        if out != 0 {
            return Err(format!("写注册表失败（错误 {out}）"));
        }
    }
    Ok(())
}

/// 写入（`disable = true`）或还原（`false`）扫描码映射。
///
/// 需要管理员权限：`HKLM` 的这个键普通用户改不了，Agent 以 High 跑才能在托盘里切换。
///
/// 写入前先备份现有表，还原时按备份精确回滚 —— 这样即使用户自己有一张改键表，
/// 关掉 L2 也只会回到他自己那张，而不是被抹成空。
pub fn set_scancode_map(disable: bool) -> Result<(), String> {
    let bak = scancode_backup_path();
    if disable {
        if !bak.exists() {
            let body = match scancode_map_raw() {
                Some(bytes) => hex_encode(&bytes),
                None => "absent".to_string(),
            };
            if let Some(dir) = bak.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Err(e) = std::fs::write(&bak, body) {
                return Err(format!(
                    "备份现有扫描码映射失败（{e}）：为免覆盖你自己的键位映射，这次不写入"
                ));
            }
        }
        write_scancode_raw(Some(&DISABLE_WIN_KEYS))
    } else {
        // 没有备份记录 = 这张表不是我们写的，别动
        let Ok(body) = std::fs::read_to_string(&bak) else {
            return Ok(());
        };
        let body = body.trim();
        if body == "absent" {
            write_scancode_raw(None)?;
        } else {
            let bytes = hex_decode(body).ok_or_else(|| "扫描码备份文件已损坏".to_string())?;
            write_scancode_raw(Some(&bytes))?;
        }
        let _ = std::fs::remove_file(&bak);
        Ok(())
    }
}

/// 我们是否正持有这张表（有备份记录）。界面用它区分「已设置待重启」和「别人设的」。
pub fn scancode_map_owned() -> bool {
    scancode_backup_path().exists()
}

/// 退出/卸载时把系统状态还回去：扫描码映射 + 无障碍热键位。
///
/// 只回滚我们确实改过的部分，没接管过的表保持原样。
pub fn restore_system_state() -> Vec<String> {
    let mut done = Vec::new();
    if scancode_map_owned() {
        match set_scancode_map(false) {
            Ok(()) => done.push("扫描码映射已还原（重启系统后 Win 键恢复）".to_string()),
            Err(e) => done.push(format!("扫描码映射还原失败：{e}")),
        }
    }
    let off = crate::catalog::ENTRIES
        .iter()
        .filter(|e| matches!(e.how, crate::catalog::How::Accessibility))
        .all(|e| set_accessibility_hotkey(e.id, false));
    if off {
        done.push("无障碍热键已恢复".to_string());
    }
    done
}

/// 当前是否已经写入了扫描码映射（用于界面显示"已设置，待重启"）。
pub fn scancode_map_present() -> bool {
    use windows_sys::Win32::System::Registry::{RegQueryValueExW, KEY_QUERY_VALUE};
    let sub = wide(SCANCODE_KEY);
    let name = wide(SCANCODE_VAL);
    let mut key: HKEY = std::ptr::null_mut();
    unsafe {
        if RegOpenKeyExW(HKEY_LOCAL_MACHINE, sub.as_ptr(), 0, KEY_QUERY_VALUE, &mut key) != 0 {
            return false;
        }
        let mut len = 0u32;
        let rc = RegQueryValueExW(
            key,
            name.as_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut len,
        );
        RegCloseKey(key);
        rc == 0 && len > 0
    }
}

// ---------------- 进程枚举（list_procs） ----------------

/// 给「新增规则」下拉框用的一条进程。
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct ProcInfo {
    /// 进程映像名（`cs2.exe`）
    pub exe: String,
    /// 主窗口标题，取不到就是空串
    pub title: String,
}

/// 枚举进程（exe 去重、按名字排序），title 取该进程某个可见顶层窗口的标题。
///
/// 只读操作：快照句柄一定关掉；任何一步失败都只是少几条，不 panic。
/// `limit` 是硬上限（协议层写 400），避免把几千条系统进程全丢给界面。
pub fn list_procs(limit: usize) -> Vec<ProcInfo> {
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    // 1) pid -> 窗口标题：EnumWindows 只认顶层可见、无 owner 的窗口
    let mut wins: Vec<(u32, String)> = Vec::new();
    unsafe {
        EnumWindows(Some(collect_window), &mut wins as *mut _ as LPARAM);
    }

    // 2) Toolhelp 列出全部进程映像名
    let mut exes: Vec<String> = Vec::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap != INVALID_HANDLE_VALUE && !snap.is_null() {
            let mut e: PROCESSENTRY32W = std::mem::zeroed();
            e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            if Process32FirstW(snap, &mut e) != 0 {
                loop {
                    let n = e.szExeFile.iter().position(|c| *c == 0).unwrap_or(e.szExeFile.len());
                    let name = String::from_utf16_lossy(&e.szExeFile[..n]);
                    if !name.is_empty() {
                        exes.push(name);
                    }
                    if Process32NextW(snap, &mut e) == 0 {
                        break;
                    }
                }
            }
            CloseHandle(snap);
        }
    }

    // 3) 合表：exe 去重 → 排序 → 截断
    let titles: Vec<(String, String)> = wins
        .into_iter()
        .filter_map(|(pid, title)| process_exe(pid).map(|exe| (exe, title)))
        .collect();
    exes.sort();
    exes.dedup();
    exes.truncate(limit);
    exes.into_iter()
        .map(|exe| {
            let title = titles
                .iter()
                .find(|(e, t)| e.eq_ignore_ascii_case(&exe) && !t.is_empty())
                .map(|(_, t)| t.clone())
                .unwrap_or_default();
            ProcInfo { exe, title }
        })
        .collect()
}

unsafe extern "system" fn collect_window(h: HWND, lp: LPARAM) -> i32 {
    if h.is_null() {
        return 1;
    }
    if IsWindowVisible(h) == 0 {
        return 1;
    }
    // 有 owner 的是对话框/工具窗，标题往往不是「这个程序在干什么」
    if !GetWindow(h, GW_OWNER).is_null() {
        return 1;
    }
    let pid = window_pid(h);
    if pid == 0 {
        return 1;
    }
    let title = window_title(h);
    if !title.is_empty() {
        let v = &mut *(lp as *mut Vec<(u32, String)>);
        v.push((pid, title));
    }
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_procs_enumerates_sorted_exes() {
        // 这条走真 Toolhelp + EnumWindows：至少能枚举到自己，且 exe 有序去重。
        let v = list_procs(400);
        assert!(!v.is_empty(), "枚举进程至少要能拿到自己");
        assert!(v.len() <= 400);
        let mut sorted = v.clone();
        sorted.sort_by(|a, b| a.exe.cmp(&b.exe));
        assert_eq!(sorted, v, "exe 必须按字典序且去重");
    }

    #[test]
    fn scancode_backup_roundtrip() {
        // 备份文件就是十六进制字节；还原时必须一个 bit 都不差，
        // 否则用户自己的改键表会被写成一张陌生的表
        let raw = [0x00u8, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x1D, 0x00, 0x3A, 0x00];
        let text = hex_encode(&raw);
        assert_eq!(text.len(), raw.len() * 2);
        assert_eq!(hex_decode(&text).unwrap(), raw);
        assert_eq!(hex_decode(&format!("{text}\r\n")).unwrap(), raw, "CRLF 要能容忍");
        assert!(hex_decode("ABC").is_none(), "奇数长度是坏文件");
        assert!(hex_decode("ZZ").is_none(), "非十六进制是坏文件");
    }

    #[test]
    fn scancode_backup_path_is_under_keyguard() {
        let p = scancode_backup_path();
        assert_eq!(p.file_name().unwrap(), "scancode.bak");
        assert_eq!(p.parent().unwrap().file_name().unwrap(), "KeyGuard");
    }
}
