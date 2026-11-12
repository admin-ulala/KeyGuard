//! 虚拟键码常量。只列产品真正要用的，避免抄一整个 winuser.h。

pub const VK_TAB: u32 = 0x09;
pub const VK_CONTROL: u32 = 0x11;
pub const VK_MENU: u32 = 0x12; // Alt
pub const VK_ESC: u32 = 0x1B;
pub const VK_SPACE: u32 = 0x20;
pub const VK_LEFT: u32 = 0x25;
pub const VK_UP: u32 = 0x26;
pub const VK_RIGHT: u32 = 0x27;
pub const VK_SNAPSHOT: u32 = 0x2C; // PrtSc

// 字母键（Win+* 组合要用）
pub const VK_A: u32 = 0x41;
pub const VK_D: u32 = 0x44;
pub const VK_E: u32 = 0x45;
pub const VK_G: u32 = 0x47;
pub const VK_I: u32 = 0x49;
pub const VK_L: u32 = 0x4C;
pub const VK_Q: u32 = 0x51;
pub const VK_R: u32 = 0x52;
pub const VK_S: u32 = 0x53;
pub const VK_U: u32 = 0x55;
pub const VK_X: u32 = 0x58;

pub const VK_LWIN: u32 = 0x5B;
pub const VK_RWIN: u32 = 0x5C;
pub const VK_F4: u32 = 0x73;
pub const VK_F8: u32 = 0x77;

pub const VK_LSHIFT: u32 = 0xA0;
pub const VK_RSHIFT: u32 = 0xA1;
pub const VK_LCONTROL: u32 = 0xA2;
pub const VK_RCONTROL: u32 = 0xA3;
pub const VK_LMENU: u32 = 0xA4;
pub const VK_RMENU: u32 = 0xA5;

/// 键名 → VK，给 CLI 与测试脚本解析用。
pub fn from_name(name: &str) -> Option<u32> {
    let n = name.trim().to_ascii_lowercase();
    Some(match n.as_str() {
        "win" => VK_LWIN,
        "alt-tab" | "alttab" => VK_TAB,
        "alt-f4" | "altf4" => VK_F4,
        "alt-esc" | "altesc" => VK_ESC,
        "ctrl-esc" | "ctrlesc" => VK_ESC,
        "prtsc" | "printscreen" => VK_SNAPSHOT,
        "win-g" | "wing" => VK_G,
        "ctrl-space" | "ctrlspace" => VK_SPACE,
        _ => return None,
    })
}
