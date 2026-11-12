//! KeyGuard 内核：钩子、按键掩码、共享状态。
//!
//! 设计约束（P1 阶段就锁死，后面不再改）：
//! 1. 回调里只做「读原子 → 位运算 → 返回」，零分配、零锁、零 IO。
//! 2. 钩子常挂，锁定与否只改一个原子量；装卸钩子有竞态，也会被 hook-scanner 当成抖动。
//! 3. 组合键靠修饰键状态位判定，不靠规则表遍历（P1 用 6 项小表，够快且好维护）。

pub mod catalog;
pub mod fullscreen;
pub mod hook;
pub mod ipc;
pub mod proto;
pub mod rules;
pub mod scenes;
pub mod mask;
pub mod state;
pub mod vk;
pub mod win;

pub use hook::{install, HookHandle};
pub use mask::KeyMask;
pub use fullscreen::{DetectCfg, Sample};
pub use ipc::sddl_for_current_user;
pub use proto::{Request, Response, StatusPayload};
pub use catalog::{Cat, Entry, How};
pub use rules::{Action, Profile, Rule, RuleSet};
pub use state::{mode_name, Foreground, Shared, Status, MODE_DEGRADED, MODE_IDLE, MODE_LOCKED, MODE_TEMP_UNLOCK};
