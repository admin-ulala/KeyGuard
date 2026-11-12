//! 前台自检 + 四态机（设计文档 §4、§5）。
//!
//! 一个专用线程每 `poll_ms` 采一次样：前台是哪个进程、它是不是真全屏。
//! 规则只按进程做判定（窗口化/无边框一样命中），全屏只是内置「全屏程序」
//! 规则里的一个可选条件。进/出都要求连续保持一段时间（迟滞），然后把结论
//! 写进 `Shared.mode`。线程本身不做任何 IO，也不碰钩子。
//!
//! 为什么不用 `SetWinEventHook`：窗口事件在游戏里是每秒几百条的量级，靠事件驱动
//! 反而要多做一次「防抖」，而且事件线程被游戏卡住时会漏事件。轮询 250ms 的代价是
//! 一次 `GetForegroundWindow` + 一次 `GetWindowRect`，实测 < 20µs。

use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::state::*;
use crate::win;

/// 这几种窗口类是桌面/托盘本身，永远不算「全屏应用」。
const SHELL_CLASSES: [&str; 5] = [
    "Progman",
    "WorkerW",
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "Windows.UI.Core.CoreWindow",
];

pub struct DetectCfg {
    /// 连续命中拦截条件多久才锁定（默认 1200ms）：避开启动器闪一下就被锁死
    pub enter: Duration,
    /// 连续不满足拦截条件多久才解除（默认 2000ms）：避开加载界面瞬间丢失前台
    pub release: Duration,
    /// 采样间隔
    pub poll: Duration,
    /// 是否自动锁定。false = 只观察不动作（诊断用）
    pub auto: bool,
}

impl Default for DetectCfg {
    fn default() -> Self {
        Self {
            enter: Duration::from_millis(1200),
            release: Duration::from_millis(2000),
            poll: Duration::from_millis(250),
            auto: true,
        }
    }
}

/// 一次采样的结论。
#[derive(Clone, Debug, Default)]
pub struct Sample {
    pub fg: Foreground,
    /// `SHQueryUserNotificationState` 原值，诊断用
    pub quns: i32,
    /// 矩形是否盖住显示器
    pub covers: bool,
    pub cloaked: bool,
}

fn take_sample(self_rid: Option<u32>, self_pid: u32) -> Sample {
    let h = win::foreground_window();
    let pid = win::window_pid(h);
    let class = win::window_class(h);
    let exe = win::process_exe(pid).unwrap_or_else(|| {
        if pid == 0 {
            String::new()
        } else {
            format!("<pid {pid}>")
        }
    });
    let quns = win::notification_state();
    let cloaked = win::is_cloaked(h);
    let covers = win::covers_monitor(h);

    let rid = if pid == 0 { None } else { win::process_rid(pid) };
    let blocked_by_uipi = match (rid, self_rid) {
        // 打不开 = 权限高于自己；RID 更高 = 一定高于自己
        (None, _) => pid != 0,
        (Some(a), Some(b)) => a > b,
        (Some(_), None) => false,
    };

    let shell = SHELL_CLASSES.contains(&class.as_str()) || pid == self_pid;

    // 判定：D3D 全屏状态直接采信；否则要求「窗口矩形盖住整块显示器」且未被 cloaked。
    // 两条都不看窗口标题/进程名 —— 那是规则引擎的事，检测层只回答「是不是全屏」。
    let fullscreen = !shell
        && !cloaked
        && (quns == 3 /* QUNS_RUNNING_D3D_FULL_SCREEN */ || covers);

    Sample {
        fg: Foreground {
            pid,
            exe,
            class,
            title: win::window_title(h),
            fullscreen,
            cloaked,
            quns,
            covers,
            rid,
            blocked_by_uipi,
        },
        quns,
        covers,
        cloaked,
    }
}

/// 启动自检线程。返回句柄；丢弃句柄不会停线程（进程退出才停），这与钩子线程一致。
pub fn spawn(st: Arc<Shared>, cfg: DetectCfg) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let self_pid = win::self_pid();
        let self_rid = win::self_rid();

        let mut fs_since: Option<Instant> = None; // 连续处于拦截条件的起点
        let mut off_since: Option<Instant> = None; // 连续不满足拦截条件的起点
        let mut session_suppressed = false; // 本次会话内被手动解除过
        let mut last_armed_by_user: Option<bool> = None;

        loop {
            std::thread::sleep(cfg.poll);
            let s = take_sample(self_rid, self_pid);
            let now = Instant::now();

            // 用户按了 F8 / 热键 / 托盘菜单「立即解除」→ 本次会话内不再自动锁
            if st.take_panic() {
                session_suppressed = true;
            }
            let armed_now = st.is_armed();
            if let Some(prev) = last_armed_by_user {
                if prev && !armed_now && st.current_mode() == MODE_LOCKED {
                    session_suppressed = true;
                }
            }
            last_armed_by_user = Some(armed_now);

            // 手动锁定：界面点了「立即锁定」，检测线程不再插手（否则下一帧就被改回待命）
            if st.is_hold() {
                st.set_foreground(s.fg.clone());
                continue;
            }

            // 仅观察模式：只更新前台信息，mode / armed 一律不碰
            // （否则 --armed --watch 会被这里按「现在不是全屏」改回待命）
            if !cfg.auto {
                st.set_foreground(s.fg.clone());
                continue;
            }

            let mode = st.current_mode();

            // 总开关关掉 = 什么都不拦。检测线程继续跑（界面还要显示前台），但不落锁定态。
            if !st.is_blocking() {
                if mode != MODE_IDLE {
                    st.set_mode(MODE_IDLE);
                }
                if !st.current_profile().is_empty() {
                    st.apply_profile(None);
                }
                fs_since = None;
                st.set_foreground(s.fg.clone());
                continue;
            }

            // ---- 规则 → 目标动作（§6）----
            // 规则判的是「前台是哪个程序」，跟窗口是否全屏无关：窗口化/无边框的游戏
            // 命中用户规则一样拦截。全屏只是内置「全屏程序」规则里的一个条件。
            let hit = {
                let rs = st.rules.read().unwrap_or_else(|e| e.into_inner());
                rs.decide(&s.fg)
            };
            let action = hit.as_ref().map(|h| h.action);
            // 场景选择：手动指定 > 规则指定 > 规则命中（process）> 全屏场景 > 常驻场景
            // 「全屏拦截」不再是写死的分支，而是 trigger == "fullscreen" 的那个场景。
            let scene = {
                let rs = st.rules.read().unwrap_or_else(|e| e.into_inner());
                let by_name = |n: &str| -> Option<crate::rules::Profile> {
                    rs.profiles
                        .iter()
                        .find(|p| p.id == n || p.name == n)
                        .cloned()
                };
                let manual = st.manual_profile();
                let from_manual = if manual.is_empty() { None } else { by_name(&manual) };
                let from_rule = hit.as_ref().and_then(|h| h.profile.clone()).and_then(|n| by_name(&n));
                let by_trigger = |t: &str| {
                    rs.profiles
                        .iter()
                        .find(|p| p.enabled && p.trigger == t)
                        .cloned()
                };
                from_manual
                    .or(from_rule)
                    .or_else(|| if hit.is_some() { by_trigger("process") } else { None })
                    .or_else(|| if s.fg.fullscreen { by_trigger("fullscreen") } else { None })
                    .or_else(|| by_trigger("always"))
            };
            let want_profile = scene.as_ref().map(|p| p.name.clone());

            let enter = hit
                .as_ref()
                .and_then(|h| h.enter_ms)
                .map(Duration::from_millis)
                .unwrap_or(cfg.enter);
            let release = hit
                .as_ref()
                .and_then(|h| h.release_ms)
                .map(Duration::from_millis)
                .unwrap_or(cfg.release);

            // 有规则命中就进拦截流程：命中「锁定」= 锁，「解除」= 临时放行，
            // 「忽略」= 什么都不做。没命中 = 让迟滞后回到待命。
            // 「常驻拦截」是独立于规则的一条通路：它不看前台是谁，永远要求锁定。
            // 注意这里不再要求「有场景可用」：没配场景时按全局键表拦，一样是拦截。
            let engaged = st.is_blocking() && (st.is_always() || crate::rules::engages(hit.as_ref()));

            let mut next = mode;

            if engaged {
                off_since = None;
                let since = *fs_since.get_or_insert(now);

                if s.fg.blocked_by_uipi && !session_suppressed {
                    // 权限不足：不能装作「已锁定」，否则用户以为拦住了
                    next = MODE_DEGRADED;
                } else if matches!(action, Some(crate::rules::Action::Unlock)) {
                    // 规则明说这个窗口要解锁（OBS 推流这类）
                    next = MODE_TEMP_UNLOCK;
                } else if session_suppressed {
                    if mode != MODE_TEMP_UNLOCK {
                        next = MODE_TEMP_UNLOCK;
                    }
                } else if now.duration_since(since) >= enter {
                    next = MODE_LOCKED;
                }
            } else {
                fs_since = None;
                let since = *off_since.get_or_insert(now);
                if now.duration_since(since) >= release {
                    session_suppressed = false; // 规则不再命中 = 会话结束，恢复自动锁定
                    next = MODE_IDLE;
                }
            }

            // ---- 场景切换 ----
            // 只有真正锁定那一刻才套场景：临时解除/降级都不该改快捷键布局，
            // 否则用户刚从游戏里 alt 出来就发现快捷键全变了。
            if next == MODE_LOCKED {
                let want = want_profile.as_deref();
                if st.current_profile().as_str() != want.unwrap_or("") {
                    st.apply_profile(want);
                }
            } else if next == MODE_IDLE {
                // 空闲时回到「手动选中的场景」，没选才回全局键表
                let keep = st.manual_profile();
                let cur = st.current_profile();
                if cur != keep {
                    if keep.is_empty() { st.apply_profile(None); } else { st.apply_profile(Some(&keep)); }
                }
            }

            // ---- 落状态 ----
            if next != mode {
                st.set_mode(next);
            }
            st.set_foreground(s.fg.clone());

            // 临时解除的剩余时间到点也由这里兜底（防止没有全屏事件时卡住）
        }
    })
}
