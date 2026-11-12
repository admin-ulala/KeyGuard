//! 共享状态。钩子回调、自检线程、CLI、将来的 IPC 服务端都读写同一个 `Shared`。
//!
//! 全部字段都是原子量：没有锁，就不会出现「回调等锁 → 超时 → 被系统摘钩子」。
//! 唯一的 `Mutex` 存的是给界面看的字符串（前台进程名等），**回调永远不碰它**。

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::RwLock;

use crate::rules::RuleSet;
use std::sync::Mutex;

use crate::mask::KeyMask;
use crate::vk;

// ---- 八项拦截能力，位标志与 UI 的八张卡片一一对应 ----
pub const F_WIN: u8 = 1 << 0; // Windows 键（整键，连带 Win+* 全部组合）
pub const F_ALT_TAB: u8 = 1 << 1;
pub const F_ALT_F4: u8 = 1 << 2;
pub const F_CTRL_ESC: u8 = 1 << 3;
pub const F_ALT_ESC: u8 = 1 << 4;
pub const F_PRTSC: u8 = 1 << 5;
pub const F_WIN_G: u8 = 1 << 6; // Xbox 游戏栏（已被 F_WIN 覆盖，单独留位给 L2 后端）
pub const F_CTRL_SPACE: u8 = 1 << 7; // 输入法切换，默认关

/// 默认开 7 项，输入法切换默认关（拦了会影响打字）。
pub const DEFAULT_FEATURES: u8 =
    F_WIN | F_ALT_TAB | F_ALT_F4 | F_CTRL_ESC | F_ALT_ESC | F_PRTSC | F_WIN_G;

pub const FEATURE_NAMES: [(&str, u8); 8] = [
    ("win", F_WIN),
    ("alt-tab", F_ALT_TAB),
    ("alt-f4", F_ALT_F4),
    ("ctrl-esc", F_CTRL_ESC),
    ("alt-esc", F_ALT_ESC),
    ("prtsc", F_PRTSC),
    ("win-g", F_WIN_G),
    ("ctrl-space", F_CTRL_SPACE),
];

// ---- 快捷键槽位布局 ----
/// 内置目录（≤64 条）+ 自定义条目（≤64 条）+ 余量 = 一次按键最多扫这么多槽。
/// 槽位表是**紧排**的：内置在前、自定义紧随其后，不留空洞（回调只扫 `0..hot_n`）。
pub const HOT_SLOTS: usize = 160;

// ---- 四态机（对应设计文档 §4） ----
pub const MODE_IDLE: u8 = 0; // 待命：当前没有命中拦截条件
pub const MODE_LOCKED: u8 = 1; // 已锁定：正在拦截
pub const MODE_DEGRADED: u8 = 2; // 降级：前台程序权限比我们高，拦不住（UIPI）
pub const MODE_TEMP_UNLOCK: u8 = 3; // 临时解除：本次游戏内不再拦，退出全屏自动恢复

pub fn mode_name(m: u8) -> &'static str {
    match m {
        MODE_LOCKED => "已锁定",
        MODE_DEGRADED => "降级·权限不足",
        MODE_TEMP_UNLOCK => "临时解除",
        _ => "待命",
    }
}

/// 回调耗时直方图：1µs 一档，0..=49 精确；≥50µs 单独计（目标 p99 < 200µs）。
pub const HIST_BUCKETS: usize = 50;
pub const HIST_STEP_US: u32 = 1;
/// ≥50µs 的样本数
pub const HIST_OVER_US: u32 = 50;

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Foreground {
    pub pid: u32,
    pub exe: String,
    pub class: String,
    pub title: String,
    pub fullscreen: bool,
    /// 窗口是否被 DWM cloaked（虚拟桌面切走、UWP 挂起）
    pub cloaked: bool,
    /// SHQueryUserNotificationState 原值，诊断用
    pub quns: i32,
    /// 窗口矩形是否盖住显示器（诊断用）
    pub covers: bool,
    /// 前台进程的完整性 RID（越高越特权）。打不开时是 None。
    pub rid: Option<u32>,
    /// 前台程序比 Agent 权限高 → UIPI 会静默吞掉我们的钩子回调
    pub blocked_by_uipi: bool,
}

#[derive(Clone, Debug)]
pub struct Status {
    pub mode: u8,
    pub armed: bool,
    pub features: u8,
    /// 当前生效的热键开关掩码（catalog 下标位）
    pub hotkeys: u64,
    /// 用户设置的全局掩码（未套用场景时与 hotkeys 相同）
    pub hotkeys_base: u64,
    /// 当前套用的场景名（没有就是空串）
    pub profile: String,
    /// 键表版本号：每次 set_hotkeys / 场景切换都会 +1，界面靠它决定要不要重取目录
    pub hotkeys_rev: u32,
    /// 执行后端：hook / scancode / off
    pub backend: String,
    /// 总开关
    pub blocking: bool,
    /// 常驻拦截：不进全屏也保持锁定
    pub always: bool,
    /// 手动场景（空 = 全局）
    pub manual: String,
    pub blocked: u64,
    pub passed: u64,
    pub alt: bool,
    pub ctrl: bool,
    pub fg_pid: u32,
    pub fg_exe: String,
    pub p50_us: u32,
    pub p99_us: u32,
    pub hook_samples: u64,
}

pub struct Shared {
    /// 四态机当前状态（0 待命 / 1 锁定 / 2 降级 / 3 临时解除）
    pub mode: AtomicU8,

    /// 是否正在拦截。等价于 `mode == MODE_LOCKED`，单独留一个原子量是为了
    /// 让回调少一次比较，也方便测试直接置位。
    pub armed: AtomicBool,
    pub features: AtomicU8,

    /// 全局热键开关（catalog 下标位）。UI 改的就是这个。
    pub hotkeys_base: AtomicU64,
    /// 生效热键开关：套了场景就是场景那份，否则等于 base。
    pub hotkeys_eff: AtomicU64,
    /// 自定义快捷键的全局开关（**custom 列表下标位**，与内置位不共享编号空间）。
    pub custom_base: AtomicU64,
    /// 自定义快捷键的生效开关（套了场景就是场景那份）。
    pub custom_eff: AtomicU64,
    /// 键表版本号：每次生效掩码变化 +1，钩子线程不需要知道细节，
    /// 但诊断/测试可以用它判断"表已经换过了"。
    pub hot_rev: AtomicU32,
    /// 钩子判定槽位：低 8 位 vk，8..12 位 need（M_ALT/M_CTRL/M_SHIFT），高位 reserved。
    /// 用固定数组而不是 Vec：回调里不能碰堆，也不能碰锁。
    ///
    /// 容量 [`HOT_SLOTS`]：内置目录在前、自定义条目紧随其后（紧排，不留空洞）。
    pub hot_slots: [AtomicU32; HOT_SLOTS],
    /// 有效槽位数（写在 slots 之后，用 Release 发布）。
    pub hot_n: AtomicU32,
    /// 当前场景名（空 = 无场景）
    pub profile: RwLock<String>,
    /// 执行后端（hook / scancode / off）。改了不即时生效，Agent 重启后才换实现。
    pub backend: RwLock<String>,

    /// 总开关：false = 完全不拦截（L1 不 armed，L2 会删掉映射表）。
    /// 这是用户唯一必须理解的开关，场景只是它下面的分层。
    pub blocking: AtomicBool,
    /// 常驻拦截（不在全屏也锁）
    pub always: AtomicBool,

    /// 手动指定的场景（空 = 全局）。规则命中的场景优先级更高，退出全屏后回到这里。
    pub manual: RwLock<String>,

    /// 修饰键当前是否按下（回调维护，组合键判定用）
    pub alt: AtomicBool,
    pub ctrl: AtomicBool,
    pub shift: AtomicBool,

    /// 已经吞掉的按下：保证对应的抬起也吞掉，避免目标程序收到「只有 up 没有 down」的脏状态
    pub held: KeyMask,

    /// 一键解除热键（默认 Ctrl+Alt+Shift+F12 的其中一部分，P1 先用 F8 方便手测）
    pub panic_vk: AtomicU32,
    pub panic_held: AtomicBool,
    pub panic_need_alt: AtomicBool,
    pub panic_need_ctrl: AtomicBool,

    /// 测试用：默认过滤掉模拟输入；置 false 后连注入的按键也参与判定，便于自动化验证
    pub ignore_injected: AtomicBool,

    /// 每个 VK 被吞的次数（256 项，2 KB）。诊断页的「最近拦截」和自动化测试都靠它。
    pub per_vk: [AtomicU64; 256],

    pub stat_blocked: AtomicU64,
    pub stat_passed: AtomicU64,

    /// 状态变化信号，给 CLI / IPC 推送用：0 无、1 锁定、2 解除
    pub signal: AtomicU8,

    // ---- 以下给「全屏自检」用，回调不读 ----
    pub fg_pid: AtomicU32,
    /// 诊断：是否启用回调耗时采样（默认开，采样本身有 ~30ns 成本）
    pub profiling: AtomicBool,
    /// 回调耗时直方图（0..49µs）
    pub hist: [AtomicU64; HIST_BUCKETS],
    /// ≥50µs 的样本数
    pub hist_over: AtomicU64,
    /// 临时解除的截止时刻（毫秒，相对进程启动）。0 = 没设。
    pub unlock_until_ms: AtomicU64,
    /// 给界面读的前台信息
    fg: Mutex<Foreground>,

    /// 手动锁定保持位：界面点了「立即锁定」后为 true，检测线程遇到它就不再改状态。
    /// 语义上等于「用户按下的开关」，只有 unlock / panic 会清掉。
    pub hold: AtomicBool,

    /// 自动规则（§6）。检测线程读、IPC 线程写；钩子回调永远不碰，避免锁进热路径。
    pub rules: RwLock<RuleSet>,

    /// 「本次全屏会话别再锁了」的一次性信号。热键 / 托盘菜单 / panic_release 都写它，
    /// 由检测线程消费后置 session_suppressed —— 绕开只改 mode 会被迟滞重新锁上的坑。
    pub panic: AtomicBool,
}

impl Default for Shared {
    fn default() -> Self {
        Self::new()
    }
}

impl Shared {
    pub fn new() -> Self {
        Self {
            mode: AtomicU8::new(MODE_IDLE),
            armed: AtomicBool::new(false),
            features: AtomicU8::new(DEFAULT_FEATURES),
            // 出厂全关：真实掩码由 Agent 启动时按 rules.json 灌进来
            hotkeys_base: AtomicU64::new(0),
            hotkeys_eff: AtomicU64::new(0),
            custom_base: AtomicU64::new(0),
            custom_eff: AtomicU64::new(0),
            hot_rev: AtomicU32::new(0),
            hot_slots: std::array::from_fn(|_| AtomicU32::new(0)),
            hot_n: AtomicU32::new(0),
            profile: RwLock::new(String::new()),
            backend: RwLock::new("hook".to_string()),
            blocking: AtomicBool::new(false),
            always: AtomicBool::new(false),
            manual: RwLock::new(String::new()),
            alt: AtomicBool::new(false),
            ctrl: AtomicBool::new(false),
            shift: AtomicBool::new(false),
            held: KeyMask::new(),
            panic_vk: AtomicU32::new(vk::VK_F8),
            panic_held: AtomicBool::new(false),
            panic_need_alt: AtomicBool::new(false),
            panic_need_ctrl: AtomicBool::new(false),
            ignore_injected: AtomicBool::new(true),
            per_vk: std::array::from_fn(|_| AtomicU64::new(0)),
            stat_blocked: AtomicU64::new(0),
            stat_passed: AtomicU64::new(0),
            signal: AtomicU8::new(0),
            fg_pid: AtomicU32::new(0),
            profiling: AtomicBool::new(true),
            hist: std::array::from_fn(|_| AtomicU64::new(0)),
            hist_over: AtomicU64::new(0),
            unlock_until_ms: AtomicU64::new(0),
            fg: Mutex::new(Foreground::default()),
            hold: AtomicBool::new(false),
            rules: RwLock::new(RuleSet::default()),
            panic: AtomicBool::new(false),
        }
    }

    #[inline]
    pub fn is_armed(&self) -> bool {
        self.armed.load(Ordering::Relaxed)
    }

    pub fn set_armed(&self, on: bool) {
        self.armed.store(on, Ordering::Relaxed);
        if !on {
            // 解除时清掉「已吞」记录，否则下次锁定第一下按键的抬起会被误吞
            self.held.clear_all();
        }
        self.signal.store(if on { 1 } else { 2 }, Ordering::Relaxed);
    }

    pub fn toggle(&self) -> bool {
        let on = !self.is_armed();
        self.set_armed(on);
        on
    }

    #[inline]
    pub fn current_mode(&self) -> u8 {
        self.mode.load(Ordering::Relaxed)
    }

    /// 换状态：唯一决定 `armed` 的地方，避免两处各写一份而不同步。
    pub fn set_mode(&self, m: u8) {
        let prev = self.mode.swap(m, Ordering::Relaxed);
        self.set_armed(m == MODE_LOCKED);
        if prev != m {
            self.signal.store(
                match m {
                    MODE_LOCKED => 1,
                    _ => 2,
                },
                Ordering::Relaxed,
            );
        }
    }

    pub fn feature_on(&self, f: u8) -> bool {
        self.features.load(Ordering::Relaxed) & f != 0
    }

    /// 按名字设置某项开关，返回是否认得出这个名字。
    pub fn set_feature(&self, name: &str, on: bool) -> bool {
        let name = name.trim().to_ascii_lowercase();
        let Some((_, bit)) = FEATURE_NAMES.iter().find(|(n, _)| *n == name) else {
            return false;
        };
        let bit = *bit;
        let mut cur = self.features.load(Ordering::Relaxed);
        loop {
            let next = if on { cur | bit } else { cur & !bit };
            match self
                .features
                .compare_exchange_weak(cur, next, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => return true,
                Err(actual) => cur = actual,
            }
        }
    }

    pub fn vk_hits(&self, vk: u32) -> u64 {
        self.per_vk[(vk as usize) & 0xFF].load(Ordering::Relaxed)
    }

    // ---- 诊断 ----

    /// 记一次回调耗时。桶下标就是微秒数，不插值。
    #[inline]
    pub fn hist_sample(&self, us: u32) {
        if us < HIST_BUCKETS as u32 {
            self.hist[us as usize].fetch_add(1, Ordering::Relaxed);
        } else {
            self.hist_over.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// 返回 (p50, p99, 样本数)，单位微秒。
    pub fn hist_percentiles(&self) -> (u32, u32, u64) {
        let mut counts = [0u64; HIST_BUCKETS];
        let mut total = 0u64;
        for (i, c) in counts.iter_mut().enumerate() {
            *c = self.hist[i].load(Ordering::Relaxed);
            total += *c;
        }
        let over = self.hist_over.load(Ordering::Relaxed);
        total += over;
        if total == 0 {
            return (0, 0, 0);
        }
        let pick = |q: f64| {
            let target = (total as f64 * q).ceil() as u64;
            let mut acc = 0u64;
            for (i, c) in counts.iter().enumerate() {
                acc += *c;
                if acc >= target {
                    return (i as u32) * HIST_STEP_US;
                }
            }
            HIST_OVER_US
        };
        (pick(0.5), pick(0.99), total)
    }

    // ---- 前台信息（自检线程写，界面读） ----

    pub fn set_foreground(&self, fg: Foreground) {
        self.fg_pid.store(fg.pid, Ordering::Relaxed);
        if let Ok(mut guard) = self.fg.lock() {
            *guard = fg;
        }
    }

    pub fn foreground(&self) -> Foreground {
        self.fg.lock().map(|g| g.clone()).unwrap_or_default()
    }

    #[inline]
    pub fn is_hold(&self) -> bool {
        self.hold.load(Ordering::Relaxed)
    }

    pub fn set_hold(&self, on: bool) {
        self.hold.store(on, Ordering::Relaxed);
    }

    /// 请求「本次会话不再拦截」。一次性信号，取走即清。
    pub fn request_panic(&self) {
        self.panic.store(true, Ordering::Release);
    }

    /// 检测线程消费信号。返回 true 表示这次要压制自动锁定。
    pub fn take_panic(&self) -> bool {
        self.panic.swap(false, Ordering::AcqRel)
    }

    /// 规则快照。检测线程每 250ms 读一次，写只发生在 IPC 线程。
    pub fn rules_snapshot(&self) -> RuleSet {
        self.rules.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// 老配置里没有场景列表时补一个「全屏应用」，否则升级后没人管全屏
    pub fn ensure_default_scene(&self) -> bool {
        let mut rs = self.rules.write().unwrap_or_else(|e| e.into_inner());
        if !rs.profiles.is_empty() {
            return false;
        }
        rs.profiles.push(crate::rules::Profile {
            id: "fullscreen".into(),
            name: "全屏应用".into(),
            trigger: "fullscreen".into(),
            enabled: true,
            // v8 起出厂全关：这个兜底场景也不预开任何快捷键
            on: Vec::new(),
        });
        true
    }

    pub fn set_rules(&self, rs: RuleSet) {
        *self.rules.write().unwrap_or_else(|e| e.into_inner()) = rs;
    }

    // ---------------- 热键目录开关（P6） ----------------

    #[inline]
    pub fn hotkeys_base(&self) -> u64 {
        self.hotkeys_base.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn hotkeys_eff(&self) -> u64 {
        self.hotkeys_eff.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn custom_base(&self) -> u64 {
        self.custom_base.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn custom_eff(&self) -> u64 {
        self.custom_eff.load(Ordering::Relaxed)
    }

    /// 两套掩码一起改：内置（catalog 位）+ 自定义（custom 位）。
    ///
    /// 拆成两个 setter 会让 rebuild 跑两遍、中间还有一瞬间只换了半套表，
    /// 所以内部统一走这一个入口。
    pub fn set_hotkeys_masks(&self, builtin: u64, custom: u64) {
        self.hotkeys_base.store(builtin, Ordering::Relaxed);
        self.custom_base.store(custom, Ordering::Relaxed);
        // 有场景在生效时不打断它；下次解锁会回到新值
        if self.profile.read().map(|p| p.is_empty()).unwrap_or(true) {
            self.hotkeys_eff.store(builtin, Ordering::Relaxed);
            self.custom_eff.store(custom, Ordering::Relaxed);
        }
        self.sync_legacy_features(builtin);
        crate::hook::rebuild(self);
    }

    /// 改全局开关：同时刷新生效掩码（没套场景时两者相同）并重建钩子表。
    pub fn set_hotkeys_base(&self, mask: u64) {
        self.set_hotkeys_masks(mask, self.custom_base());
    }

    /// 只改自定义那套（内置保持不变）。
    pub fn set_custom_base(&self, mask: u64) {
        self.set_hotkeys_masks(self.hotkeys_base(), mask);
    }

    /// 按 id 开关一条（界面单点切换用）。内置 id 走 catalog，`x:` 开头走自定义。
    pub fn toggle_hotkey(&self, id: &str, on: bool) -> bool {
        if id.starts_with(crate::rules::CUSTOM_PREFIX) {
            let i = match self.rules.read() {
                Ok(rs) => match rs.custom_index(id) {
                    Some(i) if i < 64 => i,
                    _ => return false,
                },
                Err(_) => return false,
            };
            let mut m = self.custom_base();
            if on {
                m |= 1 << i;
            } else {
                m &= !(1 << i);
            }
            self.set_custom_base(m);
            return true;
        }
        let Some(i) = crate::catalog::index_of(id) else {
            return false;
        };
        if i >= 64 {
            return false;
        }
        let mut m = self.hotkeys_base();
        if on {
            m |= 1 << i;
        } else {
            m &= !(1 << i);
        }
        self.set_hotkeys_base(m);
        true
    }

    /// 总开关。
    pub fn is_always(&self) -> bool {
        self.always.load(Ordering::Relaxed)
    }

    pub fn set_always(&self, on: bool) {
        self.always.store(on, Ordering::Relaxed);
    }

    pub fn is_blocking(&self) -> bool {
        self.blocking.load(Ordering::Relaxed)
    }

    pub fn set_blocking(&self, on: bool) {
        self.blocking.store(on, Ordering::Relaxed);
        if !on {
            // 关掉就别留着"已锁定"的假象
            self.set_mode(MODE_IDLE);
            // 「手动锁定」的钉子也要一起拔掉：否则下次再启用拦截时，
            // 检测线程会因为 hold 还是 true 而整段跳过，界面看着待命、其实永远不会自动锁。
            self.set_hold(false);
        }
    }

    pub fn manual_profile(&self) -> String {
        self.manual.read().map(|p| p.clone()).unwrap_or_default()
    }

    pub fn set_manual_profile(&self, name: Option<&str>) {
        let want = name.unwrap_or("").trim().to_string();
        if let Ok(mut cur) = self.manual.write() {
            *cur = want;
        }
    }

    /// 套用场景：传 None / 空串表示回到全局设置。
    ///
    /// 场景里存的是"绝对开关集合"而不是增量 —— 用户想的是
    /// "打这个游戏时就该这样"，不是"在全局基础上再关掉两条"。
    pub fn apply_profile(&self, name: Option<&str>) -> bool {
        let want = name.unwrap_or("").to_string();
        let (builtin, custom) = if want.is_empty() {
            // 回全局：两套都回到 base
            (self.hotkeys_base(), self.custom_base())
        } else {
            let rs = match self.rules.read() {
                Ok(g) => g,
                Err(_) => return false,
            };
            match rs.profiles.iter().find(|p| p.name == want || p.id == want) {
                // 场景的 on 里可能同时含内置 id 和 x: 自定义 id，分别按各自的编号空间取位
                Some(p) => (
                    crate::rules::mask_of(&p.on),
                    crate::rules::custom_mask_of(&rs, &p.on),
                ),
                None => return false,
            }
        };

        if let Ok(mut cur) = self.profile.write() {
            *cur = if want.is_empty() { String::new() } else { want };
        }
        self.hotkeys_eff.store(builtin, Ordering::Relaxed);
        self.custom_eff.store(custom, Ordering::Relaxed);
        crate::hook::rebuild(self);
        true
    }

    pub fn current_profile(&self) -> String {
        self.profile.read().map(|p| p.clone()).unwrap_or_default()
    }

    /// 当前执行后端（hook / scancode / off）
    pub fn backend(&self) -> String {
        self.backend.read().map(|b| b.clone()).unwrap_or_else(|_| "hook".into())
    }

    /// 只写内存。真正换实现要重启 Agent，所以这里不做任何副作用。
    pub fn set_backend(&self, name: &str) {
        if let Ok(mut b) = self.backend.write() {
            *b = name.to_string();
        }
    }

    /// 老接口的 8 位能力掩码只为兼容 CLI 与旧状态显示，从新掩码反推。
    fn sync_legacy_features(&self, mask: u64) {
        const MAP: [(&str, u8); 8] = [
            ("win", F_WIN),
            ("alt-tab", F_ALT_TAB),
            ("alt-f4", F_ALT_F4),
            ("ctrl-esc", F_CTRL_ESC),
            ("alt-esc", F_ALT_ESC),
            ("prtsc", F_PRTSC),
            ("win-g", F_WIN_G),
            ("ctrl-space", F_CTRL_SPACE),
        ];
        let mut f = 0u8;
        for (id, bit) in MAP {
            if let Some(i) = crate::catalog::index_of(id) {
                if i < 64 && mask & (1 << i) != 0 {
                    f |= bit;
                }
            }
        }
        self.features.store(f, Ordering::Relaxed);
    }

    pub fn set_feature_mask(&self, mask: u8) {
        self.features.store(mask, Ordering::Relaxed);
    }

    pub fn status(&self) -> Status {
        let (p50, p99, n) = self.hist_percentiles();
        let fg = self.foreground();
        Status {
            mode: self.current_mode(),
            armed: self.armed.load(Ordering::Relaxed),
            features: self.features.load(Ordering::Relaxed),
            hotkeys: self.hotkeys_eff(),
            hotkeys_base: self.hotkeys_base(),
            profile: self.current_profile(),
            hotkeys_rev: self.hot_rev.load(std::sync::atomic::Ordering::Relaxed),
            backend: self.backend(),
            blocking: self.is_blocking(),
            always: self.is_always(),
            manual: self.manual_profile(),
            blocked: self.stat_blocked.load(Ordering::Relaxed),
            passed: self.stat_passed.load(Ordering::Relaxed),
            alt: self.alt.load(Ordering::Relaxed),
            ctrl: self.ctrl.load(Ordering::Relaxed),
            fg_pid: fg.pid,
            fg_exe: fg.exe,
            p50_us: p50,
            p99_us: p99,
            hook_samples: n,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 「停用拦截」必须把三样东西一起归零：blocking、mode、hold。
    // 少清 hold 的话，下次启用时检测线程会整段跳过（fullscreen.rs 里 is_hold 的早退分支），
    // 表现为界面写着待命、规则却永远不生效。
    #[test]
    fn stop_blocking_clears_manual_hold() {
        let st = Shared::new();
        st.set_blocking(true);
        st.set_hold(true);
        st.set_mode(MODE_LOCKED);

        st.set_blocking(false);

        assert!(!st.is_blocking());
        assert!(!st.is_hold(), "手动锁定的钉子要拔掉");
        assert_eq!(st.current_mode(), MODE_IDLE);
    }

    #[test]
    fn enable_blocking_leaves_hold_alone() {
        let st = Shared::new();
        st.set_hold(true);
        st.set_blocking(true);
        assert!(st.is_hold(), "只在关的时候清，开的时候不碰");
    }
}
