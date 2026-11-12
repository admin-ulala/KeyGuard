//! KeyGuard 界面进程（P4）。
//!
//! 定位：**只做显示与转发**。界面永远以 Medium 权限运行，不装钩子、不碰注册表、
//! 不 Import 任何注入类 API —— 所有动作都是一条 JSON 通过命名管道发给 High 的 Agent。
//! 这条边界是产品安全性的基础：界面被换掉、被调试，也拿不到内核级权限。
//!
//! 线程模型：全部在 UI 线程。管道调用是亚毫秒级的本地 IPC，用 `WaitUntil` 定时器
//! 每 800ms 拉一次状态推给前端，比跨线程共享 WebView 简单得多，也不会有「界面已析构、
//! 推送线程还在跑」的崩溃。

// GUI 子系统：不加这行，从快捷方式启动会先弹一个黑色控制台窗口（标题是 exe 全路径）。
// 加了之后进程没有控制台，诊断信息改用 `log_line()` 落到 %APPDATA%\KeyGuard\ui.log。
#![windows_subsystem = "windows"]

use std::time::{Duration, Instant};

use tao::dpi::LogicalSize;
use tao::event::{Event, StartCause, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::window::{Window, WindowBuilder};
use wry::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use wry::WebViewBuilder;
#[cfg(windows)]
use tao::platform::windows::WindowExtWindows;
#[cfg(windows)]
use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};

mod instance;

const INDEX_HTML: &str = include_str!("../assets/index.html");
const TICK: Duration = Duration::from_millis(800);

/// 把编译期版本号/构建标记灌进页面。
///
/// 界面是 `include_str!` 进来的，版本号写在 HTML 里就会跟二进制脱钩 ——
/// 装了旧包、看着新界面，用户没法判断「这次重装到底生效没有」。
/// 这里改成构建时注入：窗口右下角那串就是**当前这个 exe** 的版本与构建时间。
fn page_html() -> String {
    let ver = env!("CARGO_PKG_VERSION");
    let build = option_env!("KG_BUILD_STAMP").unwrap_or("dev");
    INDEX_HTML
        .replace("__KG_VER__", ver)
        .replace("__KG_BUILD__", build)
}

/// 追加一行诊断日志到 `%APPDATA%\KeyGuard\ui.log`。
///
/// 界面是 GUI 子系统，没有控制台：`eprintln!` 在无控制台时写 stderr 会拿到无效句柄，
/// 不是默默失败就是直接 panic，所以这类进程一律不往 stderr 写。
/// 放 `%APPDATA%` 是跟 `rules.json` 同一个姿势 —— 卸载程序本来就会保留这个目录。
fn log_line(msg: &str) {
    use std::io::Write;

    let base = std::env::var_os("APPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join("KeyGuard");
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("ui.log"))
    {
        let _ = writeln!(f, "[{}] {msg}", now_stamp());
    }
}

/// `2026-11-09 23:50:01` 这种能人读的时间戳。
///
/// 一开始拿 `SystemTime` 自己除，出来的是 UTC（差了 8 小时），日志对不上现实；
/// 直接问系统要本地时间最省事。
fn now_stamp() -> String {
    use windows_sys::Win32::Foundation::SYSTEMTIME;
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;

    let mut st = SYSTEMTIME {
        wYear: 0,
        wMonth: 0,
        wDayOfWeek: 0,
        wDay: 0,
        wHour: 0,
        wMinute: 0,
        wSecond: 0,
        wMilliseconds: 0,
    };
    unsafe { GetLocalTime(&mut st) };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond
    )
}

fn main() -> wry::Result<()> {
    // 只允许一个界面进程；已经在跑就把那个窗口拉到前台，自己退出。
    if !instance::acquire() {
        instance::focus_existing();
        return Ok(());
    }

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();

    // 任务栏图标：assets/icon64.rgba 是 64x64 的原始 RGBA（由 docs/logo/v4-kmark.svg 导出）
    let win_icon = tao::window::Icon::from_rgba(
        include_bytes!("../../../assets/icon64.rgba").to_vec(),
        64,
        64,
    )
    .ok();

    let window = WindowBuilder::new()
        .with_title("KeyGuard")
        .with_window_icon(win_icon)
        .with_inner_size(LogicalSize::new(1080.0, 700.0))
        .with_min_inner_size(LogicalSize::new(860.0, 560.0))
        .with_decorations(false)
        .build(&event_loop)
        .expect("创建窗口失败");

    round_corners(&window);
    dark_titlebar(&window);

    let proxy = event_loop.create_proxy();
    let webview = WebViewBuilder::new()
        .with_html(match std::env::var("KEYGUARD_PAGE") {
            // 截图/测试用：直接开到某一页，省得每次都点导航
            Ok(pg) => page_html().replace("<script>", &format!("<script>window.__KG_PAGE='{pg}';</script><script>")),
            Err(_) => page_html(),
        })
        // 调试版才开 F12 检查器；发布版留着等于把内部状态和协议暴露给任何能坐到你电脑前的人
        .with_devtools(cfg!(debug_assertions))
        .with_ipc_handler(move |req| {
            let msg = req.body().to_string();
            // 窗口按钮既可能来裸串（post），也可能被 JSON 编成 "close"（send）。
            // 两种都认，免得一个引号就把关窗口变成一串协议解析错误。
            let key = msg.trim().trim_matches('"');
            let ev = match key {
                "drag" => UserEvent::Drag,
                "minimize" => UserEvent::Minimize,
                "close" => UserEvent::Exit,
                // 关闭确认里的「收进托盘」：只把窗口藏起来，进程与状态轮询都留着
                "hide" => UserEvent::Hide,
                // 界面已经把确认框弹出来了，撤销「没回话就兜底退出」的定时器
                "quit-ack" => UserEvent::QuitAck,
                _ => UserEvent::Cmd(msg),
            };
            let _ = proxy.send_event(ev);
        })
        .build(&window)
        .expect("创建 WebView 失败");

    let mut next_tick = Instant::now();
    let mut offline = false;
    // 关闭确认的兜底期限：界面脚本崩了/没加载完时 2 秒后照旧退出，
    // 否则窗口会变成点 ✕ 没反应、也关不掉的僵尸。
    let mut quit_deadline: Option<Instant> = None;

    event_loop.run(move |event, _, control_flow| {
        match event {
            Event::NewEvents(StartCause::Init) => {}
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                // Alt+F4 / 任务栏「关闭窗口」：不直接退，先让界面问一句
                quit_deadline = Some(Instant::now() + Duration::from_millis(2000));
                let _ = webview.evaluate_script(
                    "window.__KG && window.__KG.onQuitRequest && window.__KG.onQuitRequest();",
                );
            }
            Event::UserEvent(UserEvent::Exit) => {
                *control_flow = ControlFlow::Exit;
                return;
            }
            Event::UserEvent(UserEvent::Hide) => {
                quit_deadline = None;
                // 藏窗口走 Win32：托盘叫回来的窗口是 instance::focus_existing 用
                // SW_SHOW 从外部显形的，tao 内部的 visible 标志还停在 false，
                // 这时候再调 set_visible(false) 因为「状态没变」被当成空操作，
                // 表现就是第二次点 ✕ 选「收进托盘」窗口纹丝不动。
                // ShowWindow 不查 tao 的内部状态，第一次和之后每一次都生效。
                #[cfg(windows)]
                unsafe {
                    // tao 的 hwnd() 是 isize，windows-sys 0.59 的 HWND 是裸指针，转一手
                    ShowWindow(window.hwnd() as *mut std::ffi::c_void, SW_HIDE);
                }
                #[cfg(not(windows))]
                window.set_visible(false);
            }
            Event::UserEvent(UserEvent::QuitAck) => quit_deadline = None,
            Event::UserEvent(UserEvent::Drag) => {
                let _ = window.drag_window();
            }
            Event::UserEvent(UserEvent::Minimize) => window.set_minimized(true),
            Event::UserEvent(UserEvent::Cmd(text)) => {
                // 界面自己的动作（不是 Agent 的协议）：先拦截
                let resp = if text.contains("\"start_agent\"") {
                    start_agent()
                } else if text.contains("\"ui_get\"") {
                    ui_get()
                } else if text.contains("\"ui_set\"") {
                    ui_set(&text)
                } else {
                    // 前端发来的是 {"op":"lock"} 这样的原文，原样转给 Agent
                    match serde_json::from_str::<kg_core::Request>(&text) {
                        Ok(req) => kg_core::ipc::call(&req).unwrap_or_else(kg_core::Response::err),
                        Err(e) => kg_core::Response::err(format!("请求格式不对: {e}")),
                    }
                };
                let body = serde_json::to_string(&resp).unwrap_or_else(|_| "{}".into());
                let _ = webview.evaluate_script(&format!("window.__KG && window.__KG.onReply({body});"));
                // 动作之后立刻刷一次状态，界面不用等下一个 tick
                next_tick = Instant::now();
            }
            _ => {}
        }

        // ---- 定时拉状态 ----
        let now = Instant::now();
        if now >= next_tick {
            next_tick = now + TICK;
            let payload = match kg_core::ipc::call(&kg_core::Request::Status) {
                Ok(resp) if resp.ok => {
                    offline = false;
                    let data = resp.data.unwrap_or(serde_json::Value::Null);
                    serde_json::json!({ "ok": true, "status": data })
                }
                Ok(resp) => serde_json::json!({
                    "ok": false,
                    "error": resp.error.unwrap_or_else(|| "未知错误".into())
                }),
                Err(e) => {
                    let first = !offline;
                    offline = true;
                    if first {
                        log_line(&format!("管道不可达: {e}"));
                    }
                    serde_json::json!({ "ok": false, "error": e })
                }
            };
            let _ = webview.evaluate_script(&format!("window.__KG && window.__KG.onStatus({payload});"));
        }
        // 关闭确认没等到界面回话就直接退
        if let Some(dl) = quit_deadline {
            if now >= dl {
                *control_flow = ControlFlow::Exit;
                return;
            }
        }
        *control_flow = ControlFlow::WaitUntil(match quit_deadline {
            Some(dl) if dl < next_tick => dl,
            _ => next_tick,
        });
    });
}

#[derive(Debug)]
enum UserEvent {
    Cmd(String),
    Drag,
    Minimize,
    Exit,
    Hide,
    QuitAck,
}

fn hwnd_of(window: &Window) -> Option<*mut core::ffi::c_void> {
    let handle = window.window_handle().ok()?;
    match handle.as_raw() {
        RawWindowHandle::Win32(w) => Some(w.hwnd.get() as *mut core::ffi::c_void),
        _ => None,
    }
}

/// DWM 圆角：Win11 一条属性，Win10 上返回错误，忽略即可。
fn round_corners(window: &Window) {
    use windows_sys::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWM_WINDOW_CORNER_PREFERENCE,
    };
    const DWMWCP_ROUND: DWM_WINDOW_CORNER_PREFERENCE = 2;
    let Some(hwnd) = hwnd_of(window) else { return };
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            &DWMWCP_ROUND as *const _ as *const core::ffi::c_void,
            std::mem::size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32,
        );
    }
}

/// 深色窗口属性：影响系统边框与阴影的取色，让自绘标题栏看起来是一体的。
fn dark_titlebar(window: &Window) {
    use windows_sys::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE};
    let Some(hwnd) = hwnd_of(window) else { return };
    let on: i32 = 1;
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
            &on as *const _ as *const core::ffi::c_void,
            std::mem::size_of::<i32>() as u32,
        );
    }
}

/// 界面上那个「启动 Agent」按钮：走计划任务起，不弹 UAC。
/// 用户没勾自启时不建任务，退化成提权启动（会弹一次 UAC）。
/// 界面的小偏好（主题、上次停留的页）—— 存在 %APPDATA%\KeyGuard\ui.json。
///
/// 为什么不放 localStorage：WebView2 用 `NavigateToString` 加载页面，源是 opaque 的，
/// `localStorage` 直接抛 SecurityError。与其在 JS 里吞异常，不如让宿主存一份文件。
fn ui_pref_path() -> Option<std::path::PathBuf> {
    std::env::var("APPDATA")
        .ok()
        .map(|p| std::path::PathBuf::from(p).join("KeyGuard").join("ui.json"))
}

fn ui_get() -> kg_core::Response {
    let data = ui_pref_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| {
            // 手动编辑过/被 PowerShell 写过 BOM 的文件不能让整份偏好作废
            let t = t.trim_start_matches('\u{feff}');
            serde_json::from_str::<serde_json::Value>(t).ok()
        })
        .unwrap_or_else(|| serde_json::json!({}));
    kg_core::Response::with(data)
}

fn ui_set(text: &str) -> kg_core::Response {
    let Some(path) = ui_pref_path() else {
        return kg_core::Response::err("找不到 %APPDATA%");
    };
    let Ok(incoming) = serde_json::from_str::<serde_json::Value>(text) else {
        return kg_core::Response::err("请求格式不对");
    };
    let mut cur = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| {
            // 手动编辑过/被 PowerShell 写过 BOM 的文件不能让整份偏好作废
            let t = t.trim_start_matches('\u{feff}');
            serde_json::from_str::<serde_json::Value>(t).ok()
        })
        .unwrap_or_else(|| serde_json::json!({}));
    if let (Some(obj), Some(src)) = (cur.as_object_mut(), incoming.as_object()) {
        for (k, v) in src {
            if k != "op" {
                obj.insert(k.clone(), v.clone());
            }
        }
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match std::fs::write(&path, cur.to_string()) {
        Ok(()) => kg_core::Response::with(cur),
        Err(e) => kg_core::Response::err(format!("写入失败: {e}")),
    }
}

fn start_agent() -> kg_core::Response {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let out = std::process::Command::new("schtasks")
        .args(["/run", "/tn", "KeyGuard Agent"])
        .creation_flags(CREATE_NO_WINDOW)
        .output();

    match out {
        Ok(o) if o.status.success() => {
            // 给 Agent 一点时间挂上管道，前端下一次轮询就能连上
            std::thread::sleep(std::time::Duration::from_millis(600));
            kg_core::Response::with(serde_json::json!({ "ok": true }))
        }
        Ok(o) => {
            // 最常见：没勾「开机自启」，压根没这个任务
            kg_core::Response::err(format!(
                "没能启动（schtasks 退出码 {}）。没勾开机自启就不会有这个任务；                 用管理员 PowerShell 跑一次 kg-agent.exe --agent 即可",
                o.status.code().unwrap_or(-1)
            ))
        }
        Err(e) => kg_core::Response::err(format!("调不起 schtasks: {e}")),
    }
}
