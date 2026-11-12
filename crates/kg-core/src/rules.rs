//! 自动规则（设计文档 §6）。
//!
//! 一条规则回答三件事：**什么窗口**（前台进程 / 窗口类）→ **做什么**（lock/ignore/unlock）
//! → **等多久**。匹配顺序自上而下，第一条命中即生效 —— 和防火墙规则一个心智模型。
//!
//! 判定的唯一标准是**前台是哪个程序**，与窗口是不是全屏无关：游戏开成窗口化/无边框，
//! 只要它在前台，规则照样命中。想让「任何全屏程序」都拦，那是内置的「全屏程序」规则
//! （`exe` 空 + `fullscreen: true`）干的事，不需要用户自己列举进程名。
//!
//! 这份结构同时也是管道协议的一部分，所以字段名就是 JSON 名，不能随手改。

use serde::{Deserialize, Serialize};

use crate::state::{Foreground, MODE_IDLE};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    /// 锁定：吞掉系统快捷键
    #[default]
    Lock,
    /// 忽略：这个窗口全屏也不算「游戏」（浏览器看视频）
    Ignore,
    /// 解锁：这个窗口在前台时临时解除（OBS 推流）
    Unlock,
}

impl Action {
    pub fn as_str(&self) -> &'static str {
        match self {
            Action::Lock => "lock",
            Action::Ignore => "ignore",
            Action::Unlock => "unlock",
        }
    }
}

fn default_true() -> bool {
    true
}

/// 内置规则的 id：唯一的出厂规则，`exe` 空 + 仅全屏 = 任意全屏程序。
pub const BUILTIN_FULLSCREEN: &str = "fullscreen";

/// 规则文件当前版本。
///
/// v6 = 规则只看前台程序，全屏只是内置规则的条件；
/// v8 = 「快捷键目录」变成「内置目录 + 用户自定义条目」，出厂默认全关（见 `Default for RuleSet`）。
/// 旧文件靠 serde default 兼容，不需要迁移函数。
pub const RULES_VERSION: u32 = 8;

/// 自定义快捷键的 id 前缀。内置目录里不会出现 `x:` 开头的 id，所以两套 id 不会撞。
pub const CUSTOM_PREFIX: &str = "x:";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Rule {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// 进程名列表（不区分大小写）。空 = 任意进程（配合其他条件才有意义）。
    #[serde(default)]
    pub exe: Vec<String>,
    /// 窗口类名，空 = 不限
    #[serde(default)]
    pub class: String,
    /// 仅当全屏时匹配。默认 false：规则只看前台是哪个程序，窗口化运行同样生效。
    /// 只有内置的「全屏程序」规则会打开它。
    #[serde(default)]
    pub fullscreen: bool,
    /// 常驻：不看前台是哪个程序，规则永远命中。
    ///
    /// 和 `fullscreen` 一样属于"附加条件"，只是它不带任何条件 —— 用它表达
    /// 「任何时候都拦」这条规则，不用去动全局的 `always` 总闸。
    #[serde(default)]
    pub always: bool,
    /// 仅当前台时匹配（默认真：规则本来就是按前台程序判的）
    #[serde(default = "default_true")]
    pub foreground: bool,
    #[serde(default)]
    pub action: Action,
    /// 进锁延迟（毫秒）。0 = 用全局默认
    #[serde(default)]
    pub delay_ms: u64,
    /// 出锁延迟（毫秒）。0 = 用全局默认
    #[serde(default)]
    pub release_delay_ms: u64,
    /// unlock 动作的持续时长（毫秒），0 = 本次会话
    #[serde(default)]
    pub ttl_ms: u64,
    /// 命中这条规则时把快捷键开关切到哪个场景（空 = 用全局设置）
    #[serde(default)]
    pub profile: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl Rule {
    /// 是不是出厂那条「全屏程序」。
    pub fn is_builtin(&self) -> bool {
        self.id == BUILTIN_FULLSCREEN
    }

    /// 是否命中这个前台窗口。
    ///
    /// 只有 `exe` / `class` 参与「是哪个程序」的判断，`fullscreen` 是可选附加条件：
    /// 内置「全屏程序」规则靠它把「任意程序」收窄成「任意全屏程序」；
    /// 用户自己加的规则默认不看全屏，窗口化运行也照拦。
    pub fn matches(&self, fg: &Foreground) -> bool {
        if !self.enabled {
            return false;
        }
        // 常驻：不参与任何前台判断，直接命中。
        if self.always {
            return true;
        }
        if !self.class.is_empty() && !eq_ignore_case(&self.class, &fg.class) {
            return false;
        }
        if !self.exe.is_empty() && !self.exe.iter().any(|e| exe_matches(e, &fg.exe)) {
            return false;
        }
        // 既没进程名、又没类名、还不限全屏 = 会命中任意前台窗口。
        // 那不是规则，是把整个系统锁死，所以当它不存在。
        if self.exe.is_empty() && self.class.is_empty() && !self.fullscreen {
            return false;
        }
        if self.fullscreen && !fg.fullscreen {
            return false;
        }
        if self.foreground && fg.pid == 0 {
            return false;
        }
        true
    }
}

fn eq_ignore_case(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.eq_ignore_ascii_case(b)
}

/// 进程名匹配。用户在界面上通常只记得一半名字（"三角洲行动" 的实际进程叫
/// `DeltaForceClient-Win64-Shipping.exe`），所以这里故意放宽：
///
/// - 写全 `xxx.exe`：严格相等（忽略大小写），不会误伤同前缀的进程
/// - 带 `*`：按通配符，例如 `*deltaforce*`
/// - 其他写法：按片段包含，例如 `deltaforce` / `shipping`
fn exe_matches(pattern: &str, exe: &str) -> bool {
    let p = pattern.trim().to_ascii_lowercase();
    let e = exe.trim().to_ascii_lowercase();
    if p.is_empty() || e.is_empty() {
        return false;
    }
    if p.contains('*') {
        let mut rest = e.as_str();
        for seg in p.split('*').filter(|s| !s.is_empty()) {
            match rest.find(seg) {
                Some(i) => rest = &rest[i + seg.len()..],
                None => return false,
            }
        }
        return true;
    }
    if p.ends_with(".exe") {
        return e == p;
    }
    e.contains(&p)
}

/// 一套命名好的快捷键开关（"场景"）。游戏场景就是命中规则时套上它。
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct Profile {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// 触发条件：always（常驻）/ fullscreen（任意程序进全屏）/ process（规则命中时）
    #[serde(default = "d_trigger")]
    pub trigger: String,
    /// 场景本身是否参与判定
    #[serde(default = "d_true")]
    pub enabled: bool,
    /// 场景下**开着**的快捷键 id（绝对集合，不是增量）
    #[serde(default)]
    pub on: Vec<String>,
}

fn d_trigger() -> String { "process".to_string() }
pub const TRIGGERS: [&str; 3] = ["fullscreen", "always", "process"];

/// 用户自定义的一条快捷键（v8）。
///
/// 它不在 `catalog::ENTRIES` 里 —— 内置目录是 `'static`，而自定义条目来自规则文件，
/// 所以单独存在 `RuleSet::custom`，用**列表下标**做位掩码（0..64）。
/// 这也意味着**删掉一条会让后面所有条目的 bit 前移**，掩码必须每次按 id 重算，
/// 不能跨版本持久化。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CustomKey {
    /// 形如 `x:ctrlatlq`，由 combo 归一化得到，避免与内置 id 冲突
    pub id: String,
    /// 用户可改的显示名，默认用 combo
    pub name: String,
    /// 展示串，如 `Ctrl + Alt + Q`
    pub combo: String,
    /// 1..=3 个 VK
    #[serde(default)]
    pub vks: Vec<u32>,
    /// `crate::catalog::{M_ALT,M_CTRL,M_SHIFT}` 的或
    #[serde(default)]
    pub need: u8,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RuleSet {
    pub version: u32,
    pub rules: Vec<Rule>,
    #[serde(default)]
    pub profiles: Vec<Profile>,
    /// 全局开着的快捷键 id（内置 id + `x:` 开头的自定义 id）
    #[serde(default)]
    pub hotkeys: Vec<String>,
    /// 用户自定义的快捷键（v8）。位掩码下标 = 它在这个 Vec 里的位置。
    #[serde(default)]
    pub custom: Vec<CustomKey>,
    /// 执行后端：hook（L1 键盘钩子）/ scancode（L2 扫描码映射，需重启）/ off（只检测不拦）
    #[serde(default = "d_backend")]
    pub backend: String,
    /// 总开关（是否开始监控全屏）。默认 false：装上不启用时完全不看全屏、不拦任何键。
    #[serde(default = "d_false")]
    pub blocking: bool,
    /// 常驻拦截：不进全屏也保持锁定
    #[serde(default)]
    pub always: bool,
}

fn d_false() -> bool {
    false
}

fn d_true() -> bool {
    true
}

fn d_backend() -> String {
    // 默认走 L1（键盘钩子）：装完就能拦，不需要重启系统。
    // L2（扫描码映射）改的是 HKLM 里的键盘驱动映射，必须重启才生效、而且只能处理单键，
    // 所以它是「在安全页手动选择」的后端 —— 拿它当出厂默认，等于默认状态下什么都不拦。
    "hook".to_string()
}

/// 出厂那条「全屏程序」规则。用户想按进程拦，自己加规则，我们不预置任何游戏。
pub fn builtin_fullscreen_rule() -> Rule {
    Rule {
        id: BUILTIN_FULLSCREEN.into(),
        name: "全屏程序".into(),
        exe: Vec::new(),
        class: String::new(),
        fullscreen: true,
        always: false,
        foreground: true,
        action: Action::Lock,
        delay_ms: 0,
        release_delay_ms: 0,
        ttl_ms: 0,
        profile: String::new(),
        enabled: true,
    }
}

impl Default for RuleSet {
    fn default() -> Self {
        // 出厂「全关」：没有规则、没有场景、没有开着的快捷键。
        //
        // 首次安装时用户什么都没配，Agent 就不该动任何按键 —— 这是最不容易吓到人的
        // 默认值。想让「全屏程序」生效，用户在规则页加一条就行（内置规则模板仍由
        // `builtin_fullscreen_rule()` 提供，只是不再预置进新配置）。
        Self {
            version: RULES_VERSION,
            backend: d_backend(),
            blocking: false,
            always: false,
            hotkeys: Vec::new(),
            custom: Vec::new(),
            profiles: Vec::new(),
            rules: Vec::new(),
        }
    }
}

/// 命中的规则 + 它带来的延迟覆盖。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub action: Action,
    /// 命中规则要求切换的场景（空 = 不切）
    pub profile: Option<String>,
    pub enter_ms: Option<u64>,
    pub release_ms: Option<u64>,
}

impl RuleSet {
    /// 自上而下，第一条命中即返回；都没命中返回 `None`（= 不干预）。
    pub fn decide(&self, fg: &Foreground) -> Option<Hit> {
        for r in &self.rules {
            if r.matches(fg) {
                return Some(Hit {
                    action: r.action,
                    profile: (!r.profile.is_empty()).then(|| r.profile.clone()),
                    enter_ms: (r.delay_ms > 0).then_some(r.delay_ms),
                    release_ms: (r.release_delay_ms > 0).then_some(r.release_delay_ms),
                });
            }
        }
        None
    }

    /// v6 迁移：把配置升级到「规则只看前台程序」。
    ///
    /// - 删掉早期内置的示例规则（浏览器 / CS2 / VALORANT / OBS）—— 游戏该让用户自己配；
    /// - 补上厂唯一的「全屏程序」规则（exe 空 + 仅全屏）；
    /// - 用户自己的规则一律清掉「仅当全屏」：判断标准是前台是什么程序，
    ///   窗口化/无边框运行同样命中；
    /// - 缺「全屏应用」场景就补一个，否则升级后没人管全屏。
    ///
    /// 返回 true 表示有改动，调用方负责落盘。
    pub fn migrate(&mut self) -> bool {
        let mut changed = false;

        // v3：总开关默认不启用
        if self.version < 3 {
            self.blocking = false;
            self.always = false;
        }

        // 早期内置的示例规则：删掉，一条都不留
        const SAMPLES: [&str; 4] = ["browser", "valorant", "obs", "cs2"];
        let before = self.rules.len();
        self.rules.retain(|r| !SAMPLES.contains(&r.id.as_str()));
        if self.rules.len() != before {
            changed = true;
        }

        // 示例场景同理（CS2 / 挂机 也是种子数据）。只删没人引用的：
        // 场景 id 是自定的，用户自己建的不在名单里，不会被碰到。
        const SAMPLE_PROFILES: [&str; 2] = ["cs2", "gacha"];
        let used: Vec<String> = self
            .rules
            .iter()
            .filter(|r| !r.profile.is_empty())
            .map(|r| r.profile.clone())
            .collect();
        let pbefore = self.profiles.len();
        self.profiles
            .retain(|p| !SAMPLE_PROFILES.contains(&p.id.as_str()) || used.contains(&p.id));
        if self.profiles.len() != pbefore {
            changed = true;
        }

        // 老的「任意全屏」规则改名成内置「全屏程序」
        if let Some(f) = self.rules.iter_mut().find(|r| r.id == "any-fullscreen") {
            f.id = BUILTIN_FULLSCREEN.into();
            f.name = "全屏程序".into();
            changed = true;
        }

        // v7：默认场景原来叫「全屏游戏」——这套规则跟游戏没关系，改成中性的「全屏应用」。
        // 只动名字还是出厂默认的那个，用户自己改过的名字不碰。
        if let Some(p) = self
            .profiles
            .iter_mut()
            .find(|p| p.id == BUILTIN_FULLSCREEN && p.name == "全屏游戏")
        {
            p.name = "全屏应用".into();
            changed = true;
        }
        for r in self.rules.iter_mut() {
            if r.profile == "全屏游戏" {
                r.profile = "全屏应用".into();
                changed = true;
            }
        }

        // 用户规则：不再看全屏，只认前台程序
        for r in self.rules.iter_mut() {
            if r.is_builtin() {
                continue;
            }
            if r.fullscreen {
                r.fullscreen = false;
                changed = true;
            }
            if !r.foreground {
                r.foreground = true;
                changed = true;
            }
        }

        // 内置规则如果还在，它的定义就是「全屏时生效」；用户往里加的进程名不动他。
        // 注意：**不再自动补**这条规则，也不再自动补 trigger=fullscreen 的场景 ——
        // v8 起允许「一条规则都没有 / 一个场景都没有」（出厂全关），
        // 自动补会把这个状态在每次加载时又变回去。
        if let Some(f) = self.rules.iter_mut().find(|r| r.is_builtin()) {
            if !f.fullscreen {
                f.fullscreen = true;
                changed = true;
            }
        }

        if self.version < RULES_VERSION {
            self.version = RULES_VERSION;
            changed = true;
        }
        changed
    }

    pub fn validate(&self) -> Result<(), String> {
        for p in &self.profiles {
            if !TRIGGERS.contains(&p.trigger.as_str()) {
                return Err(format!("场景 {} 的触发条件未知: {}", p.id, p.trigger));
            }
        }
        if !matches!(self.backend.as_str(), "hook" | "scancode" | "off") {
            return Err(format!("未知的执行后端: {}", self.backend));
        }
        let mut seen = std::collections::HashSet::new();
        if self.rules.len() > 64 {
            return Err("规则数量超过 64 条".into());
        }
        for (i, r) in self.rules.iter().enumerate() {
            if r.id.is_empty() {
                return Err(format!("第 {} 条规则缺少 id", i + 1));
            }
            if !seen.insert(r.id.clone()) {
                return Err(format!("规则 id 重复: {}", r.id));
            }
            if r.exe.iter().any(|e| e.trim().is_empty()) {
                return Err(format!("规则 {} 的 exe 列表里有空项", r.id));
            }
            // 内置 id 只是历史遗留的「出厂模板」标记：用户在界面上改它的条件
            // （例如把「全屏程序」改成「当前台是 xxx.exe」）是正当操作，不拦。
            if !r.profile.is_empty() && !self.profiles.iter().any(|p| p.name == r.profile || p.id == r.profile) {
                return Err(format!("规则 {} 引用了不存在的场景: {}", r.id, r.profile));
            }
            if r.delay_ms > 30_000 || r.release_delay_ms > 30_000 {
                return Err(format!("规则 {} 的延迟超过 30s", r.id));
            }
        }

        // ---- 自定义快捷键 ----
        if self.custom.len() > 64 {
            return Err("自定义快捷键超过 64 条".into());
        }
        let mut seen_custom = std::collections::HashSet::new();
        for c in &self.custom {
            if !c.id.starts_with(CUSTOM_PREFIX) || c.id.len() == CUSTOM_PREFIX.len() {
                return Err(format!("自定义快捷键 id 必须以 {} 开头: {}", CUSTOM_PREFIX, c.id));
            }
            if !seen_custom.insert(c.id.clone()) {
                return Err(format!("自定义快捷键 id 重复: {}", c.id));
            }
            if c.vks.is_empty() {
                return Err(format!("自定义快捷键 {} 缺少按键", c.id));
            }
            if c.vks.len() > 3 {
                return Err(format!("自定义快捷键 {} 最多 3 个按键", c.id));
            }
            if c.vks.iter().any(|v| *v == 0 || *v > 0xFF) {
                return Err(format!("自定义快捷键 {} 的按键码超出 1..255", c.id));
            }
            if c.need & !(crate::catalog::M_ALT | crate::catalog::M_CTRL | crate::catalog::M_SHIFT) != 0 {
                return Err(format!("自定义快捷键 {} 的修饰键掩码无效", c.id));
            }
        }
        Ok(())
    }
}

/// 规则结论 → 该不该进拦截流程。
///
/// `Lock` / `Unlock` 都算「命中」——拦截流程内部再区分锁死还是临时放行；
/// `Ignore` 与没命中一样，都不拦（已锁的会走释放迟滞退回待命）。
pub fn engages(hit: Option<&Hit>) -> bool {
    matches!(hit.map(|h| h.action), Some(Action::Lock) | Some(Action::Unlock))
}

/// 目标模式：把规则结论翻成状态机该去的态。
pub fn target_mode(hit: Option<Hit>, current: u8) -> u8 {
    match hit.map(|h| h.action) {
        Some(Action::Lock) => crate::state::MODE_LOCKED,
        Some(Action::Ignore) => crate::state::MODE_IDLE,
        Some(Action::Unlock) => crate::state::MODE_TEMP_UNLOCK,
        // 没规则命中：保持现状，由检测线程的迟滞决定
        None => current,
    }
}

const _: u8 = MODE_IDLE;

// ---------------- 快捷键集合 <-> 位掩码 ----------------

/// id 列表 → 位掩码。未知 id 静默忽略（老配置文件里有已删除的条目属正常）。
pub fn mask_of(ids: &[String]) -> u64 {
    let mut m = 0u64;
    for id in ids {
        if let Some(i) = crate::catalog::index_of(id) {
            if i < 64 {
                m |= 1 << i;
            }
        }
    }
    m
}

/// 位掩码 → id 列表，按目录顺序输出（写配置文件用，保证 diff 稳定）。
pub fn ids_of(mask: u64) -> Vec<String> {
    let mut v = Vec::new();
    for (i, e) in crate::catalog::ENTRIES.iter().enumerate() {
        if i < 64 && mask & (1 << i) != 0 {
            v.push(e.id.to_string());
        }
    }
    v
}

// ---- 自定义快捷键 <-> 位掩码（bit = custom 列表下标） ----

/// combo → 归一化串：小写、去空白、去 `+`。`"Ctrl + Alt + Q"` → `"ctrlaltq"`。
pub fn normalize_combo(combo: &str) -> String {
    combo
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '+')
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// combo → 自定义条目 id（带 `x:` 前缀）。
pub fn custom_id(combo: &str) -> String {
    format!("{CUSTOM_PREFIX}{}", normalize_combo(combo))
}

/// id 列表 → 自定义掩码。不属于 `rs.custom` 的 id 静默忽略。
pub fn custom_mask_of(rs: &RuleSet, ids: &[String]) -> u64 {
    let mut m = 0u64;
    for id in ids {
        if let Some(i) = rs.custom_index(id) {
            if i < 64 {
                m |= 1 << i;
            }
        }
    }
    m
}

/// 自定义掩码 → id 列表（按 `custom` 顺序）。
pub fn custom_ids_of(rs: &RuleSet, mask: u64) -> Vec<String> {
    rs.custom
        .iter()
        .take(64)
        .enumerate()
        .filter(|(i, _)| mask & (1 << i) != 0)
        .map(|(_, c)| c.id.clone())
        .collect()
}

/// 内置 + 自定义合并成一条 id 列表：写 `rules.json` 的 `hotkeys`、给界面渲染都靠它。
pub fn ids_with_custom(rs: &RuleSet, builtin: u64, custom: u64) -> Vec<String> {
    let mut v = ids_of(builtin);
    v.extend(custom_ids_of(rs, custom));
    v
}

impl RuleSet {
    /// 自定义 id → 它在 `custom` 里的下标（= 掩码位）。
    pub fn custom_index(&self, id: &str) -> Option<usize> {
        self.custom.iter().position(|c| c.id == id)
    }

    /// 追加一条自定义快捷键，返回生成的 id。
    ///
    /// 校验放在这里而不是只放 `validate()`：IPC 收到请求时要立刻给界面一句能看懂的话。
    pub fn add_custom(&mut self, name: &str, combo: &str, need: u8, vks: &[u32]) -> Result<String, String> {
        let combo = combo.trim();
        if combo.is_empty() {
            return Err("快捷键组合不能为空".into());
        }
        if vks.is_empty() {
            return Err("至少要指定一个按键".into());
        }
        if vks.len() > 3 {
            return Err("一个自定义快捷键最多 3 个按键".into());
        }
        if let Some(bad) = vks.iter().find(|v| **v == 0 || **v > 0xFF) {
            return Err(format!("无效的按键码: {bad}"));
        }
        if need & !(crate::catalog::M_ALT | crate::catalog::M_CTRL | crate::catalog::M_SHIFT) != 0 {
            return Err("无效的修饰键掩码".into());
        }
        if self.custom.len() >= 64 {
            return Err("自定义快捷键最多 64 条".into());
        }
        let id = custom_id(combo);
        if id == CUSTOM_PREFIX {
            return Err("快捷键组合不能为空".into());
        }
        if self.custom.iter().any(|c| c.id == id) {
            return Err(format!("这个组合已经加过了: {combo}"));
        }
        let name = name.trim();
        let name = if name.is_empty() { combo.to_string() } else { name.to_string() };
        self.custom.push(CustomKey {
            id: id.clone(),
            name,
            combo: combo.to_string(),
            vks: vks.to_vec(),
            need,
        });
        Ok(id)
    }

    /// 删一条自定义快捷键：同时把它从全局开关和**所有场景**里剔掉，
    /// 否则场景里会留下一个永远打不开的死 id。
    pub fn remove_custom(&mut self, id: &str) -> bool {
        let before = self.custom.len();
        self.custom.retain(|c| c.id != id);
        if self.custom.len() == before {
            return false;
        }
        self.hotkeys.retain(|h| h != id);
        for p in self.profiles.iter_mut() {
            p.on.retain(|h| h != id);
        }
        true
    }
}

#[cfg(test)]
mod exe_match_tests {
    use super::exe_matches;

    #[test]
    fn full_name_is_exact() {
        assert!(exe_matches("DeltaForceClient.exe", "deltaforceclient.exe"));
        assert!(!exe_matches("DeltaForceClient.exe", "DeltaForceClient-Win64-Shipping.exe"));
    }

    #[test]
    fn fragment_matches_long_name() {
        assert!(exe_matches("deltaforce", "DeltaForceClient-Win64-Shipping.exe"));
        assert!(exe_matches("shipping", "DeltaForceClient-Win64-Shipping.exe"));
        assert!(!exe_matches("cs2", "DeltaForceClient-Win64-Shipping.exe"));
    }

    #[test]
    fn wildcard_globs() {
        assert!(exe_matches("*deltaforce*", "DeltaForceClient-Win64-Shipping.exe"));
        assert!(exe_matches("deltaforce*shipping.exe", "DeltaForceClient-Win64-Shipping.exe"));
        assert!(!exe_matches("*shipping*deltaforce*", "DeltaForceClient-Win64-Shipping.exe"));
    }

    #[test]
    fn empty_never_matches() {
        assert!(!exe_matches("", "anything.exe"));
        assert!(!exe_matches("a.exe", ""));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog;

    fn fg(exe: &str, fullscreen: bool) -> Foreground {
        Foreground {
            pid: 100,
            exe: exe.into(),
            class: "GameWindow".into(),
            title: String::new(),
            fullscreen,
            ..Default::default()
        }
    }

    fn user_rule(exe: &str) -> Rule {
        Rule {
            id: "my-game".into(),
            name: "我的游戏".into(),
            exe: vec![exe.into()],
            fullscreen: false,
            action: Action::Lock,
            enabled: true,
            ..builtin_fullscreen_rule()
        }
    }

    /// 出厂「全关」：规则的默认值、场景、快捷键、自定义条目全都空。
    fn rs_with_builtin() -> RuleSet {
        let mut rs = RuleSet::default();
        rs.rules.push(builtin_fullscreen_rule());
        rs
    }

    #[test]
    fn default_is_all_off() {
        let rs = RuleSet::default();
        assert_eq!(rs.version, RULES_VERSION);
        assert!(rs.rules.is_empty(), "出厂不预置任何规则");
        assert!(rs.profiles.is_empty(), "出厂不预置任何场景");
        assert!(rs.hotkeys.is_empty(), "出厂不打开任何快捷键");
        assert!(rs.custom.is_empty());
        assert!(!rs.blocking);
        assert!(!rs.always);
        assert_eq!(rs.backend, "hook");
    }

    #[test]
    fn validate_accepts_empty_ruleset() {
        // 空配置是合法状态：首次安装就是这个
        assert!(RuleSet::default().validate().is_ok());
    }

    #[test]
    fn old_rules_json_without_custom_still_loads() {
        // v7 的文件没有 custom 字段，靠 serde(default) 兼容，不写迁移函数
        let old = r#"{"version":7,"rules":[{"id":"my-game","name":"我的游戏","exe":["game.exe"],"action":"lock","profile":"游戏"}],"profiles":[{"id":"youxi","name":"游戏","trigger":"process","on":["win"]}],"hotkeys":["win"],"backend":"hook","blocking":true,"always":false}"#;
        let mut rs: RuleSet = serde_json::from_str(old).expect("旧文件必须能解析");
        assert!(rs.custom.is_empty());
        assert_eq!(rs.rules.len(), 1);
        rs.migrate();
        assert_eq!(rs.version, RULES_VERSION, "加载后升到 v8");
        assert!(rs.validate().is_ok());
    }

    #[test]
    fn custom_id_is_normalized_and_prefixed() {
        assert_eq!(normalize_combo("Ctrl + Alt + Q"), "ctrlaltq");
        assert_eq!(custom_id("Ctrl + Alt + Q"), "x:ctrlaltq");
        assert_eq!(custom_id(" Shift+F1 "), "x:shiftf1");
        // 大小写/空格/加号不同的写法必须落到同一个 id，否则会加出重复条目
        assert_eq!(custom_id("ctrl+alt+q"), custom_id("CTRL ALT Q"));
    }

    #[test]
    fn add_custom_uses_combo_as_default_name() {
        let mut rs = RuleSet::default();
        let id = rs.add_custom("", "Ctrl + Alt + Q", catalog::M_CTRL | catalog::M_ALT, &[0x51])
            .expect("应能添加");
        assert_eq!(id, "x:ctrlaltq");
        assert_eq!(rs.custom.len(), 1);
        assert_eq!(rs.custom[0].name, "Ctrl + Alt + Q", "name 空就用 combo");
        assert_eq!(rs.custom[0].vks, vec![0x51]);
        assert_eq!(rs.custom[0].need, 3);
        assert!(rs.validate().is_ok());
        // 重复组合要被挡住
        assert!(rs.add_custom("再来一次", "Ctrl + Alt + Q", 3, &[0x51]).is_err());
    }

    #[test]
    fn add_custom_rejects_bad_input() {
        let mut rs = RuleSet::default();
        assert!(rs.add_custom("", "  ", 0, &[0x51]).is_err(), "空组合");
        assert!(rs.add_custom("x", "Ctrl+Q", 0, &[]).is_err(), "没有按键");
        assert!(
            rs.add_custom("x", "Ctrl+Q", 0, &[1, 2, 3, 4]).is_err(),
            "超过 3 个键"
        );
        assert!(rs.add_custom("x", "Ctrl+Q", 0, &[0]).is_err(), "vk 0 无效");
        assert!(rs.add_custom("x", "Ctrl+Q", 0, &[0x100]).is_err(), "vk 超界");
        assert!(rs.add_custom("x", "Ctrl+Q", 0x80, &[0x51]).is_err(), "need 含非法位");
        assert!(rs.custom.is_empty(), "校验失败不能留下半条数据");
    }

    #[test]
    fn custom_mask_follows_list_position() {
        let mut rs = RuleSet::default();
        rs.add_custom("Q", "Ctrl + Alt + Q", 3, &[0x51]).unwrap();
        rs.add_custom("W", "Ctrl + Alt + W", 3, &[0x57]).unwrap();
        let m = custom_mask_of(&rs, &["x:ctrlaltw".to_string()]);
        assert_eq!(m, 1 << 1, "bit = custom 下标");
        assert_eq!(custom_ids_of(&rs, m), vec!["x:ctrlaltw"]);
        assert_eq!(
            ids_with_custom(&rs, 0, 0b11),
            vec!["x:ctrlaltq", "x:ctrlaltw"]
        );
        // 删掉第一条后，第二条的位要跟着下移（下标即位置）
        assert!(rs.remove_custom("x:ctrlaltq"));
        assert_eq!(custom_mask_of(&rs, &["x:ctrlaltw".to_string()]), 1);
    }

    #[test]
    fn remove_custom_prunes_scene_references() {
        let mut rs = RuleSet::default();
        let id = rs.add_custom("Q", "Ctrl + Alt + Q", 3, &[0x51]).unwrap();
        rs.hotkeys.push(id.clone());
        rs.profiles.push(Profile {
            id: "p".into(),
            name: "场景".into(),
            trigger: "process".into(),
            enabled: true,
            on: vec![id.clone()],
        });
        assert!(rs.remove_custom(&id));
        assert!(rs.custom.is_empty());
        assert!(rs.hotkeys.is_empty(), "全局开关里也要剔掉");
        assert!(rs.profiles[0].on.is_empty(), "场景里也要剔掉");
        assert!(!rs.remove_custom("x:never"), "删不存在的返回 false");
    }

    #[test]
    fn validate_rejects_bad_custom_entries() {
        let mut rs = RuleSet::default();
        rs.custom.push(CustomKey {
            id: "no-prefix".into(),
            name: "x".into(),
            combo: "Ctrl+Q".into(),
            vks: vec![0x51],
            need: 2,
        });
        assert!(rs.validate().is_err(), "id 必须带 x: 前缀");

        let mut rs = RuleSet::default();
        rs.custom.push(CustomKey {
            id: "x:q".into(),
            name: "x".into(),
            combo: "Ctrl+Q".into(),
            vks: vec![],
            need: 2,
        });
        assert!(rs.validate().is_err(), "vks 不能为空");

        let mut rs = RuleSet::default();
        rs.custom.push(CustomKey {
            id: "x:q".into(),
            name: "x".into(),
            combo: "Ctrl+Q".into(),
            vks: vec![0x51],
            need: 8,
        });
        assert!(rs.validate().is_err(), "need 只能含 ALT/CTRL/SHIFT");
    }

    #[test]
    fn default_backend_installs_the_hook() {
        // 出厂必须是 L1：L2 要重启系统才生效，拿它当默认等于装完什么都不拦
        let rs = RuleSet::default();
        assert_eq!(rs.backend, "hook");
        assert!(!rs.blocking, "出厂不自动接管键盘");
    }

    #[test]
    fn fullscreen_rule_locks_any_fullscreen_app() {
        let rs = rs_with_builtin();
        let hit = rs.decide(&fg("anything.exe", true)).expect("应命中全屏程序");
        assert_eq!(hit.action, Action::Lock);
    }

    #[test]
    fn windowed_app_does_not_match_fullscreen_rule() {
        let rs = rs_with_builtin();
        assert!(rs.decide(&fg("mygame.exe", false)).is_none());
    }

    #[test]
    fn user_rule_matches_windowed_game() {
        // 关键回归：用户配了进程规则，窗口化运行也必须命中
        let mut rs = RuleSet::default();
        rs.rules.insert(0, user_rule("mygame.exe"));
        let hit = rs.decide(&fg("mygame.exe", false)).expect("窗口化也要命中");
        assert_eq!(hit.action, Action::Lock);
        assert!(rs.decide(&fg("other.exe", false)).is_none());
    }

    #[test]
    fn user_rule_matches_case_insensitive() {
        let mut rs = RuleSet::default();
        rs.rules.insert(0, user_rule("mygame.exe"));
        assert!(rs.decide(&fg("MYGAME.EXE", false)).is_some());
    }

    #[test]
    fn always_rule_matches_every_foreground() {
        // 「常驻」：没有任何进程名/全屏条件，也必须命中任意前台
        let mut rs = RuleSet::default();
        let mut r = user_rule("");
        r.exe.clear();
        r.always = true;
        rs.rules.push(r);
        assert!(rs.decide(&fg("explorer.exe", false)).is_some());
        assert!(rs.decide(&fg("cs2.exe", true)).is_some());
        assert!(rs.decide(&fg("", false)).is_some());

        // 关掉这条规则就不再命中
        rs.rules[0].enabled = false;
        assert!(rs.decide(&fg("explorer.exe", false)).is_none());
    }

    #[test]
    fn always_rule_survives_validate_and_roundtrip() {
        // 旧文件没有 always 字段 → 默认 false；常驻规则存盘后必须还能读回来
        let mut rs = RuleSet::default();
        let mut r = user_rule("");
        r.exe.clear();
        r.always = true;
        rs.rules.push(r);
        rs.validate().expect("常驻规则应当合法");
        let txt = serde_json::to_string(&rs).unwrap();
        let back: RuleSet = serde_json::from_str(&txt).unwrap();
        assert!(back.rules[0].always);
        let old = r#"{"version":8,"rules":[{"id":"r1","name":"x","exe":["a.exe"],"action":"lock"}]}"#;
        let parsed: RuleSet = serde_json::from_str(old).unwrap();
        assert!(!parsed.rules[0].always);
    }

    #[test]
    fn empty_rule_never_matches() {
        // exe/class 都空、又不限全屏 = 会锁死一切，直接当它不存在
        let mut rs = rs_with_builtin();
        rs.rules[0].fullscreen = false;
        assert!(rs.decide(&fg("chrome.exe", false)).is_none());
        assert!(rs.decide(&fg("chrome.exe", true)).is_none());
    }

    #[test]
    fn disabled_rule_is_skipped() {
        let mut rs = rs_with_builtin();
        rs.rules[0].enabled = false;
        assert!(rs.decide(&fg("chrome.exe", true)).is_none());
    }

    #[test]
    fn migrate_rules_and_scenes() {
        // 完整还原一份「装过示例包」的 v5 规则：4 条示例规则 + 示例场景 + 用户自己的一条
        let json = r#"{"version":5,"rules":[
            {"id":"browser","name":"浏览器全屏","exe":["chrome.exe"],"fullscreen":true,"action":"ignore"},
            {"id":"valorant","name":"VALORANT","exe":["VALORANT-Win64-Shipping.exe"],"fullscreen":true},
            {"id":"cs2","name":"CS2","exe":["cs2.exe"],"fullscreen":true,"profile":"cs2"},
            {"id":"obs","name":"OBS","exe":["obs64.exe"],"fullscreen":true,"action":"unlock"},
            {"id":"mine","name":"我的游戏","exe":["game.exe"],"fullscreen":true,"profile":"myown"}
        ],"profiles":[
            {"id":"fullscreen","name":"全屏应用","trigger":"fullscreen","on":["win"]},
            {"id":"cs2","name":"CS2","trigger":"process","on":["w"]},
            {"id":"gacha","name":"挂机","trigger":"process","on":[]},
            {"id":"myown","name":"我的场景","trigger":"process","on":["tab"]}
        ]}"#;
        let mut rs: RuleSet = serde_json::from_str(json).unwrap();
        assert!(rs.migrate());
        assert_eq!(rs.version, RULES_VERSION);
        let ids: Vec<&str> = rs.rules.iter().map(|r| r.id.as_str()).collect();
        // v8 起不再自动补内置「全屏程序」规则：用户文件里没有就保持没有
        assert_eq!(ids, vec!["mine"]);
        let mine = &rs.rules[0];
        assert!(!mine.fullscreen, "用户规则不再看全屏");
        assert!(mine.foreground);
        // 示例场景跟着示例规则一起走；用户自己建的（哪怕没规则引用）保留
        let scenes: Vec<&str> = rs.profiles.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(scenes, vec!["fullscreen", "myown"]);
    }

    #[test]
    fn migrate_keeps_sample_scene_still_referenced() {
        // 用户把示例场景留着并挂在自己的规则上：场景不能删，否则规则指向空气
        let json = r#"{"version":5,"rules":[
            {"id":"mine","name":"我的","exe":["game.exe"],"profile":"cs2"}
        ],"profiles":[{"id":"cs2","name":"CS2","trigger":"process","on":[]}]}"#;
        let mut rs: RuleSet = serde_json::from_str(json).unwrap();
        assert!(rs.migrate());
        assert!(rs.profiles.iter().any(|p| p.id == "cs2"));
    }

    #[test]
    fn migrate_is_idempotent() {
        let mut rs: RuleSet = serde_json::from_str(r#"{"version":5,"rules":[]}"#).unwrap();
        assert!(rs.migrate());
        assert!(!rs.migrate(), "第二次迁移不该再改东西");
    }

    #[test]
    fn migrate_renames_game_scene_to_neutral_name() {
        // v6 的默认场景叫「全屏游戏」，v7 改成「全屏应用」
        let mut rs: RuleSet = serde_json::from_str(
            r#"{"version":6,"rules":[{"id":"fullscreen","name":"全屏程序","exe":[],"class":"","fullscreen":true,"foreground":true,"action":"lock","delay_ms":0,"release_delay_ms":0,"ttl_ms":0,"profile":"全屏游戏","enabled":true}],
                "profiles":[{"id":"fullscreen","name":"全屏游戏","trigger":"fullscreen","enabled":true,"on":["win"]}]}"#,
        )
        .unwrap();
        assert!(rs.migrate());
        assert_eq!(rs.profiles[0].name, "全屏应用");
        assert_eq!(rs.rules[0].profile, "全屏应用");
        assert!(!rs.migrate(), "改名只做一次");
    }

    #[test]
    fn validate_rejects_duplicate_id() {
        let mut rs = RuleSet::default();
        rs.rules.push(user_rule("a.exe"));
        rs.rules.push(user_rule("b.exe"));
        rs.rules[1].id = "my-game".into();
        assert!(rs.validate().is_err());
    }

    #[test]
    fn validate_allows_editing_builtin_rule() {
        // 用户可以在界面上把「全屏程序」改成别的条件，保存时不该被拦
        let mut rs = rs_with_builtin();
        rs.rules[0].fullscreen = false;
        rs.rules[0].exe = vec!["cs2.exe".into()];
        assert!(rs.validate().is_ok());
    }

    #[test]
    fn ignore_rule_before_builtin_wins() {
        // 「浏览器全屏不拦」= 用户自己加一条 ignore 排在内置规则前面
        let mut rs = rs_with_builtin();
        rs.rules.insert(
            0,
            Rule { action: Action::Ignore, ..user_rule("chrome.exe") },
        );
        assert_eq!(rs.decide(&fg("chrome.exe", true)).unwrap().action, Action::Ignore);
        assert_eq!(rs.decide(&fg("mygame.exe", true)).unwrap().action, Action::Lock);
    }

    #[test]
    fn engages_only_lock_or_unlock() {
        let mk = |a: Action| Hit { action: a, profile: None, enter_ms: None, release_ms: None };
        assert!(engages(Some(&mk(Action::Lock))));
        assert!(engages(Some(&mk(Action::Unlock))));
        assert!(!engages(Some(&mk(Action::Ignore))));
        assert!(!engages(None));
    }

    #[test]
    fn json_roundtrip_keeps_defaults() {
        let json = r#"{"version":1,"rules":[{"id":"a","name":"A","exe":["x.exe"]}]}"#;
        let rs: RuleSet = serde_json::from_str(json).unwrap();
        assert_eq!(rs.rules[0].action, Action::Lock);
        assert!(rs.rules[0].foreground);
        assert!(rs.rules[0].enabled);
        assert!(!rs.rules[0].fullscreen);
    }
}
