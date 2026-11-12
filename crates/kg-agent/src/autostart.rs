//! 开机自启：计划任务（AtLogOn + 最高权限）。
//!
//! 为什么不用注册表 `Run` 键：那条路只能以 Medium 启动，Agent 也就拦不住
//! 管理员权限的游戏（UIPI）。计划任务可以带 `/rl highest`，登录即 High。
//!
//! 用 `schtasks` 而不是 COM 的 `ITaskService`：少一次 COM 初始化与几百行样板，
//! 代价是依赖系统盘里的 schtasks.exe —— 它在所有 Win10/11 上都有。

use std::os::windows::process::CommandExt;
use std::process::Command;

const TASK: &str = "KeyGuard Agent";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn schtasks(args: &[&str]) -> std::io::Result<std::process::Output> {
    Command::new("schtasks")
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
}

/// 计划任务是否已存在。
pub fn is_enabled() -> bool {
    match schtasks(&["/query", "/tn", TASK]) {
        Ok(o) => o.status.success(),
        Err(_) => false,
    }
}

/// 建任务：登录时触发、最高权限、延迟 3 秒（等 explorer 与托盘就绪）。
pub fn enable() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("拿不到自身路径：{e}"))?;
    let tr = format!("\"{}\" --agent", exe.display());

    let out = schtasks(&[
        "/create",
        "/tn",
        TASK,
        "/tr",
        &tr,
        "/sc",
        "onlogon",
        "/rl",
        "highest",
        "/delay",
        "0000:03",
        "/f",
    ])
    .map_err(|e| format!("调不起 schtasks：{e}"))?;

    if out.status.success() {
        Ok(())
    } else {
        Err(describe_failure(&out))
    }
}

pub fn disable() -> Result<(), String> {
    let out = schtasks(&["/delete", "/tn", TASK, "/f"]).map_err(|e| format!("调不起 schtasks：{e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(describe_failure(&out))
    }
}

/// 失败原因说人话：拒绝访问 = 需要管理员。
fn describe_failure(out: &std::process::Output) -> String {
    let raw = decode(&out.stderr)
        .or_else(|| decode(&out.stdout))
        .unwrap_or_default();
    let code = out.status.code().unwrap_or(-1);
    if raw.contains("拒绝访问") || raw.contains("Access is denied") || code == 1 {
        return format!("需要管理员权限（Agent 以 High 运行时可自行创建）。schtasks: {raw}");
    }
    if raw.is_empty() {
        format!("schtasks 退出码 {code}")
    } else {
        raw
    }
}

/// schtasks 的输出是控制台代码页（简中系统为 GBK），这里只做尽力而为的解码：
/// 让用户在 MessageBox 里看到能认的中文。
fn decode(bytes: &[u8]) -> Option<String> {
    crate::console::console_text(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_name_is_stable() {
        assert_eq!(TASK, "KeyGuard Agent");
    }
}
