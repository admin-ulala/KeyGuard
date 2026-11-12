//! 快捷键目录。
//!
//! 这里只放"事实"：这条快捷键是谁、什么组合、用什么手段能拦、默认开不开。
//! 判定逻辑在 `hook.rs`（键级）和 `win.rs`（辅助功能开关）里，UI 只负责渲染这张表。
//!
//! 为什么不在 UI 里硬编码：条目要参与三个地方 —— 钩子建表、注册表类开关、
//! 以及规则文件里的 id 引用。三处共用一份 static，改一处不会漏。

/// 需要 Alt 按住。
pub const M_ALT: u8 = 1;
/// 需要 Ctrl 按住。
pub const M_CTRL: u8 = 2;
/// 需要 Shift 按住。
pub const M_SHIFT: u8 = 4;

/// 拦截手段。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum How {
    /// 低级键盘钩子直接吞掉（唯一对全屏独占游戏也有效的办法）。
    Hook,
    /// 被"Win 键整键拦截"连带解决，不能单独开关。
    WinCover,
    /// Windows 无障碍开关：走 SystemParametersInfo，可随时还原。
    Accessibility,
    /// 拦不住，只作为说明列出（安全序列 / 系统桌面）。
    Never,
}

/// 分组只影响 UI 排序。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cat {
    System,
    Window,
    Game,
    Input,
    Access,
    Danger,
}

impl Cat {
    pub fn title(self) -> &'static str {
        match self {
            Cat::System => "系统与开始菜单",
            Cat::Window => "窗口与任务切换",
            Cat::Game => "截屏与录制",
            Cat::Input => "输入法",
            Cat::Access => "辅助功能（粘滞/筛选/切换键）",
            Cat::Danger => "拦不住的",
        }
    }
}

pub struct Entry {
    /// 稳定 id：写进 rules.json，改名会让用户的配置丢项，所以别改。
    pub id: &'static str,
    pub cat: Cat,
    pub name: &'static str,
    /// 给人看的组合写法。
    pub combo: &'static str,
    /// 钩子要匹配的虚拟键（Win 键是左右两个分码）。
    pub vks: &'static [u32],
    /// 必须同时按住的修饰键（`M_*` 的或）。
    pub need: u8,
    pub how: How,
    /// 一句后果说明，UI 显示在副标题。
    pub note: &'static str,
    /// 默认是否勾选。
    pub default_on: bool,
}

use crate::vk;

/// 目录。顺序即 UI 顺序（同组内保持书写顺序）。
pub static ENTRIES: &[Entry] = &[
    // ---------- 系统与开始菜单 ----------
    Entry {
        id: "win",
        cat: Cat::System,
        name: "Windows 键",
        combo: "Win",
        vks: &[vk::VK_LWIN, vk::VK_RWIN],
        need: 0,
        how: How::Hook,
        note: "整键拦截：开始菜单与全部 Win 组合键一起失效",
        default_on: true,
    },
    Entry {
        id: "win-e",
        cat: Cat::System,
        name: "打开资源管理器",
        combo: "Win + E",
        vks: &[vk::VK_E],
        need: 0,
        how: How::WinCover,
        note: "随 Win 键一起被拦",
        default_on: true,
    },
    Entry {
        id: "win-r",
        cat: Cat::System,
        name: "运行对话框",
        combo: "Win + R",
        vks: &[vk::VK_R],
        need: 0,
        how: How::WinCover,
        note: "随 Win 键一起被拦（这条最常被误触）",
        default_on: true,
    },
    Entry {
        id: "win-d",
        cat: Cat::System,
        name: "显示桌面",
        combo: "Win + D",
        vks: &[vk::VK_D],
        need: 0,
        how: How::WinCover,
        note: "随 Win 键一起被拦，切走前台最难受的一条",
        default_on: true,
    },
    Entry {
        id: "win-i",
        cat: Cat::System,
        name: "设置",
        combo: "Win + I",
        vks: &[vk::VK_I],
        need: 0,
        how: How::WinCover,
        note: "随 Win 键一起被拦",
        default_on: true,
    },
    Entry {
        id: "win-s",
        cat: Cat::System,
        name: "搜索",
        combo: "Win + S",
        vks: &[vk::VK_S],
        need: 0,
        how: How::WinCover,
        note: "随 Win 键一起被拦",
        default_on: true,
    },
    Entry {
        id: "win-a",
        cat: Cat::System,
        name: "快速设置",
        combo: "Win + A",
        vks: &[vk::VK_A],
        need: 0,
        how: How::WinCover,
        note: "随 Win 键一起被拦",
        default_on: true,
    },
    Entry {
        id: "win-x",
        cat: Cat::System,
        name: "高级用户菜单",
        combo: "Win + X",
        vks: &[vk::VK_X],
        need: 0,
        how: How::WinCover,
        note: "随 Win 键一起被拦",
        default_on: true,
    },
    Entry {
        id: "win-l",
        cat: Cat::System,
        name: "锁屏",
        combo: "Win + L",
        vks: &[vk::VK_L],
        need: 0,
        how: How::WinCover,
        note: "随 Win 键一起被拦；挂钩子之前这条是拦不住的",
        default_on: true,
    },
    Entry {
        id: "win-u",
        cat: Cat::System,
        name: "轻松使用中心",
        combo: "Win + U",
        vks: &[vk::VK_U],
        need: 0,
        how: How::WinCover,
        note: "随 Win 键一起被拦",
        default_on: true,
    },
    Entry {
        id: "win-q",
        cat: Cat::System,
        name: "搜索（旧版）",
        combo: "Win + Q",
        vks: &[vk::VK_Q],
        need: 0,
        how: How::WinCover,
        note: "随 Win 键一起被拦",
        default_on: true,
    },
    // ---------- 窗口与任务切换 ----------
    Entry {
        id: "alt-tab",
        cat: Cat::Window,
        name: "任务切换",
        combo: "Alt + Tab",
        vks: &[vk::VK_TAB],
        need: M_ALT,
        how: How::Hook,
        note: "切走前台最常见的一条",
        default_on: true,
    },
    Entry {
        id: "alt-f4",
        cat: Cat::Window,
        name: "关闭当前窗口",
        combo: "Alt + F4",
        vks: &[vk::VK_F4],
        need: M_ALT,
        how: How::Hook,
        note: "误触会直接关掉当前窗口",
        default_on: true,
    },
    Entry {
        id: "ctrl-esc",
        cat: Cat::System,
        name: "开始菜单（Ctrl+Esc）",
        combo: "Ctrl + Esc",
        vks: &[vk::VK_ESC],
        need: M_CTRL,
        how: How::Hook,
        note: "老式开始菜单快捷键，Alt+Tab 之外最常见的一条",
        default_on: true,
    },
    Entry {
        id: "alt-esc",
        cat: Cat::Window,
        name: "窗口轮换",
        combo: "Alt + Esc",
        vks: &[vk::VK_ESC],
        need: M_ALT,
        how: How::Hook,
        note: "不发任务切换界面，直接把下一个窗口拉到前台",
        default_on: true,
    },
    Entry {
        id: "alt-space",
        cat: Cat::Window,
        name: "窗口系统菜单",
        combo: "Alt + Space",
        vks: &[vk::VK_SPACE],
        need: M_ALT,
        how: How::Hook,
        note: "会呼出窗口菜单并把画面缩回窗口",
        default_on: true,
    },
    Entry {
        id: "win-tab",
        cat: Cat::Window,
        name: "任务视图",
        combo: "Win + Tab",
        vks: &[vk::VK_TAB],
        need: 0,
        how: How::WinCover,
        note: "随 Win 键一起被拦",
        default_on: true,
    },
    Entry {
        id: "win-ctrl-d",
        cat: Cat::Window,
        name: "新建虚拟桌面",
        combo: "Win + Ctrl + D",
        vks: &[vk::VK_D],
        need: 0,
        how: How::WinCover,
        note: "随 Win 键一起被拦，否则画面会直接换走",
        default_on: true,
    },
    Entry {
        id: "win-ctrl-left",
        cat: Cat::Window,
        name: "切换虚拟桌面",
        combo: "Win + Ctrl + ← / →",
        vks: &[vk::VK_LEFT, vk::VK_RIGHT],
        need: 0,
        how: How::WinCover,
        note: "随 Win 键一起被拦",
        default_on: true,
    },
    Entry {
        id: "win-left",
        cat: Cat::Window,
        name: "窗口贴靠",
        combo: "Win + ← / → / ↑",
        vks: &[vk::VK_LEFT, vk::VK_RIGHT, vk::VK_UP],
        need: 0,
        how: How::WinCover,
        note: "随 Win 键一起被拦",
        default_on: true,
    },
    // ---------- 截屏与录制 ----------
    Entry {
        id: "prtsc",
        cat: Cat::Game,
        name: "屏幕截图",
        combo: "PrtSc",
        vks: &[vk::VK_SNAPSHOT],
        need: 0,
        how: How::Hook,
        note: "按一下就把截图塞进剪贴板，独占全屏下还可能掉帧",
        default_on: true,
    },
    Entry {
        id: "win-prtsc",
        cat: Cat::Game,
        name: "截图并保存",
        combo: "Win + PrtSc",
        vks: &[vk::VK_SNAPSHOT],
        need: 0,
        how: How::WinCover,
        note: "随 Win 键一起被拦",
        default_on: true,
    },
    Entry {
        id: "win-shift-s",
        cat: Cat::Game,
        name: "截图工具",
        combo: "Win + Shift + S",
        vks: &[vk::VK_S],
        need: 0,
        how: How::WinCover,
        note: "随 Win 键一起被拦",
        default_on: true,
    },
    Entry {
        id: "win-g",
        cat: Cat::Game,
        name: "Xbox 游戏栏",
        combo: "Win + G",
        vks: &[vk::VK_G],
        need: 0,
        how: How::WinCover,
        note: "覆盖层 + 录制提示，半透明层会一直挂在画面上",
        default_on: true,
    },
    Entry {
        id: "win-alt-r",
        cat: Cat::Game,
        name: "开始/停止录制",
        combo: "Win + Alt + R",
        vks: &[vk::VK_R],
        need: 0,
        how: How::WinCover,
        note: "随 Win 键一起被拦",
        default_on: true,
    },
    // ---------- 输入法 ----------
    Entry {
        id: "ctrl-space",
        cat: Cat::Input,
        name: "中英切换",
        combo: "Ctrl + Space",
        vks: &[vk::VK_SPACE],
        need: M_CTRL,
        how: How::Hook,
        note: "默认放行：拦了就没法打字，需要时再开",
        default_on: false,
    },
    Entry {
        id: "win-space",
        cat: Cat::Input,
        name: "切换输入法",
        combo: "Win + Space",
        vks: &[vk::VK_SPACE],
        need: 0,
        how: How::WinCover,
        note: "随 Win 键一起被拦",
        default_on: true,
    },
    // ---------- 辅助功能 ----------
    Entry {
        id: "sticky-keys",
        cat: Cat::Access,
        name: "粘滞键",
        combo: "Shift × 5",
        vks: &[],
        need: 0,
        how: How::Accessibility,
        note: "连按五下 Shift 弹窗，会把前台窗口切走",
        default_on: true,
    },
    Entry {
        id: "filter-keys",
        cat: Cat::Access,
        name: "筛选键",
        combo: "右 Shift 按住 8 秒",
        vks: &[],
        need: 0,
        how: How::Accessibility,
        note: "长按右 Shift 触发；长时间按住 Shift 容易误触",
        default_on: true,
    },
    Entry {
        id: "toggle-keys",
        cat: Cat::Access,
        name: "切换键",
        combo: "NumLock 按住 5 秒",
        vks: &[],
        need: 0,
        how: How::Accessibility,
        note: "长按 NumLock 弹提示音",
        default_on: true,
    },
    Entry {
        id: "high-contrast",
        cat: Cat::Access,
        name: "高对比度",
        combo: "左 Alt + 左 Shift + PrtSc",
        vks: &[],
        need: 0,
        how: How::Accessibility,
        note: "触发后整个画面配色翻转，退出很麻烦",
        default_on: true,
    },
    // ---------- 拦不住的 ----------
    Entry {
        id: "ctrl-alt-del",
        cat: Cat::Danger,
        name: "安全注意序列",
        combo: "Ctrl + Alt + Del",
        vks: &[],
        need: 0,
        how: How::Never,
        note: "由内核直接交给 Winlogon，任何用户态钩子都收不到；它也是这个工具最后的逃生门",
        default_on: false,
    },
    Entry {
        id: "uac-desktop",
        cat: Cat::Danger,
        name: "UAC 安全桌面",
        combo: "触发 UAC 时",
        vks: &[],
        need: 0,
        how: How::Never,
        note: "提示框在独立桌面上，钩子不可达；一般不会出现",
        default_on: false,
    },
];

/// 按 id 查。
pub fn find(id: &str) -> Option<&'static Entry> {
    ENTRIES.iter().find(|e| e.id == id)
}

/// 目录里下标（用于位掩码）。
pub fn index_of(id: &str) -> Option<usize> {
    ENTRIES.iter().position(|e| e.id == id)
}

/// 能被独立开关的条目数（Hook + Accessibility），位掩码按这个上限设计。
pub fn toggleable() -> usize {
    ENTRIES
        .iter()
        .filter(|e| matches!(e.how, How::Hook | How::Accessibility))
        .count()
}

/// 默认掩码：下标与 `ENTRIES` 一致，只使用能被开关的条目。
pub fn default_mask() -> u64 {
    let mut m = 0u64;
    for (i, e) in ENTRIES.iter().enumerate() {
        if i >= 64 {
            break;
        }
        if e.default_on && matches!(e.how, How::Hook | How::Accessibility) {
            m |= 1 << i;
        }
    }
    m
}

/// 全开（导入/重置时用）。
pub fn full_mask() -> u64 {
    let mut m = 0u64;
    for (i, e) in ENTRIES.iter().enumerate() {
        if i >= 64 {
            break;
        }
        if matches!(e.how, How::Hook | How::Accessibility) {
            m |= 1 << i;
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for e in ENTRIES {
            assert!(seen.insert(e.id), "重复 id: {}", e.id);
        }
    }

    #[test]
    fn hook_entries_have_keys() {
        for e in ENTRIES {
            if e.how == How::Hook {
                assert!(!e.vks.is_empty(), "{} 是 Hook 类却没有 vk", e.id);
            }
        }
    }

    #[test]
    fn every_toggleable_has_a_bit() {
        assert!(
            ENTRIES.len() <= 64,
            "目录条目 {} 超过 64，掩码要换 u128",
            ENTRIES.len()
        );
        let m = default_mask();
        // 默认至少包含 win / alt-tab / alt-f4 / prtsc
        for id in ["win", "alt-tab", "alt-f4", "prtsc"] {
            let i = index_of(id).expect(id);
            assert_ne!(m & (1 << i), 0, "{id} 默认应该是开的");
        }
        // 输入法切换默认关
        let i = index_of("ctrl-space").expect("ctrl-space");
        assert_eq!(m & (1 << i), 0, "ctrl-space 默认应该关");
    }

    #[test]
    fn never_and_wincover_are_not_toggleable() {
        for e in ENTRIES {
            if matches!(e.how, How::Never | How::WinCover) {
                let i = index_of(e.id).unwrap();
                assert_eq!(full_mask() & (1 << i), 0, "{} 不该有开关位", e.id);
            }
        }
    }
}
