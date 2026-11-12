//! 管道协议（设计文档 §13.2）。
//!
//! 六个动词，一个信封。所有消息都是**一行 JSON**，以消息模式管道的一条完整消息收发。
//! 只暴露这六个动词是有意的：管道是 High 进程的攻击面，能少一个入口就少一个。

use serde::{Deserialize, Serialize};

use crate::rules::RuleSet;
use crate::state::{mode_name, Foreground, Status};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// 立即锁定
    Lock,
    /// 立即解除（回到待命，检测线程会重新判断）
    Unlock,
    /// 读状态
    Status,
    GetRules,
    PutRules { rules: RuleSet },
    /// 本次全屏会话内不再拦截，并且在进程内清掉「已吞」记录
    PanicRelease,
    /// 开机自启：读状态（on 为空）或设置（on 有值）
    Autostart { on: Option<bool> },
    /// 改八项能力掩码（旧接口，保留给 CLI）
    SetFeatures { mask: u8 },
    /// 读快捷键目录 + 当前开关（快捷键页）
    GetCatalog,
    /// 整体替换快捷键开关（按 catalog id）
    SetHotkeys { ids: Vec<String> },
    /// 导出全部配置（规则 + 场景 + 快捷键）为一段 JSON 文本
    ExportRules,
    /// 导入配置：整段 JSON 覆盖，校验不过就整份拒绝
    ImportRules { json: String },
    /// 新建/覆盖一个场景（该场景下**开着**的快捷键 id 全量集合）
    SetScene { name: String, ids: Vec<String>, #[serde(default)] trigger: Option<String> },
    /// 删除场景（引用它的规则退回"不切场景"）
    DeleteScene { name: String },
    /// 切换执行后端：hook（L1 钩子）/ scancode（L2 扫描码映射）/ off（只检测不拦）
    SetBackend { name: String },
    /// 总开关：false = 完全停用（L1 不 armed，L2 删映射表）
    SetBlocking { on: bool },
    /// 常驻拦截：不在全屏也锁
    SetAlways { on: bool },
    /// 手动选场景（None / 空串 = 全局）。规则命中的场景优先级更高。
    SetProfile { name: Option<String> },
    /// 枚举有主窗口的进程（给「新增规则」的下拉框用）
    ListProcs,
    /// 新增一条自定义快捷键。`name` 空则用 `combo`。
    /// `need` 是 M_ALT|M_CTRL|M_SHIFT 的或，`vks` 为 1..=3 个虚拟键码。
    AddCustomKey {
        #[serde(default)]
        name: String,
        combo: String,
        #[serde(default)]
        need: u8,
        vks: Vec<u32>,
    },
    /// 删一条自定义快捷键（同时从 hotkeys 与所有场景里剔除）
    RemoveCustomKey { id: String },
    /// 场景改名：id 按 slug 重算，引用它的规则一起改写
    RenameProfile { from: String, to: String },
    /// 复制一份场景
    DupProfile { from: String, to: String },
}

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl Response {
    pub fn ok() -> Self {
        Self { ok: true, error: None, data: None }
    }
    pub fn with<T: Serialize>(data: T) -> Self {
        Self {
            ok: true,
            error: None,
            data: serde_json::to_value(data).ok(),
        }
    }
    pub fn err(msg: impl Into<String>) -> Self {
        Self { ok: false, error: Some(msg.into()), data: None }
    }
}

/// `status` 动词的返回体。字段名直接喂给界面，别乱改。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StatusPayload {
    pub agent_pid: u32,
    pub elevated: bool,
    pub mode: u8,
    pub mode_name: String,
    pub armed: bool,
    pub features: u8,
    pub blocked: u64,
    pub passed: u64,
    pub fg: Foreground,
    pub p50_us: u32,
    pub p99_us: u32,
    pub hook_samples: u64,
    /// 执行后端：hook / scancode / off
    #[serde(default)]
    pub backend: String,
    /// L2 的扫描码映射是否已写入注册表（写了就要重启才生效）
    #[serde(default)]
    pub scancode_map: bool,
    /// 总开关
    #[serde(default = "yes")]
    pub blocking: bool,
    /// 手动场景（空 = 全局）
    #[serde(default)]
    pub manual: String,
    #[serde(default)]
    pub hotkeys_rev: u32,
    #[serde(default)]
    pub always: bool,
    /// 后台能力清单：界面靠它判断后台是不是新版（缺这个字段的就是旧版）。
    #[serde(default)]
    pub caps: Vec<String>,
    pub pipe_sddl: String,
}

impl StatusPayload {
    pub fn from_status(s: &Status, fg: Foreground, sddl: String) -> Self {
        Self {
            agent_pid: std::process::id(),
            elevated: crate::win::is_elevated(),
            mode: s.mode,
            mode_name: mode_name(s.mode).to_string(),
            armed: s.armed,
            features: s.features,
            blocked: s.blocked,
            passed: s.passed,
            fg,
            p50_us: s.p50_us,
            p99_us: s.p99_us,
            hook_samples: s.hook_samples,
            backend: s.backend.clone(),
            scancode_map: crate::win::scancode_map_present(),
            blocking: s.blocking,
            manual: s.manual.clone(),
            hotkeys_rev: s.hotkeys_rev,
            always: s.always,
            caps: CAPS.iter().map(|c| c.to_string()).collect(),
            pipe_sddl: sddl,
        }
    }
}

/// 本版本支持的新增能力。界面用 `caps` 里有没有某项来决定要不要显示对应 UI。
pub const CAPS: [&str; 4] = ["custom_keys", "list_procs", "profile_meta", "always_rule"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_tags() {
        let j = serde_json::to_string(&Request::Lock).unwrap();
        assert_eq!(j, r#"{"op":"lock"}"#);
        let r: Request = serde_json::from_str(r#"{"op":"panic_release"}"#).unwrap();
        assert!(matches!(r, Request::PanicRelease));
    }

    #[test]
    fn put_rules_parses() {
        let j = r#"{"op":"put_rules","rules":{"version":1,"rules":[{"id":"x","name":"X","exe":["a.exe"],"action":"unlock"}]}}"#;
        let r: Request = serde_json::from_str(j).unwrap();
        match r {
            Request::PutRules { rules } => {
                assert_eq!(rules.rules.len(), 1);
                assert_eq!(rules.rules[0].action, crate::rules::Action::Unlock);
            }
            _ => panic!("解析成了别的动词"),
        }
    }

    #[test]
    fn new_ops_parse() {
        let r: Request =
            serde_json::from_str(r#"{"op":"list_procs"}"#).unwrap();
        assert!(matches!(r, Request::ListProcs));

        let r: Request = serde_json::from_str(
            r#"{"op":"add_custom_key","name":"","combo":"Ctrl + Alt + Q","need":3,"vks":[81]}"#,
        )
        .unwrap();
        match r {
            Request::AddCustomKey { name, combo, need, vks } => {
                assert!(name.is_empty());
                assert_eq!(combo, "Ctrl + Alt + Q");
                assert_eq!(need, 3);
                assert_eq!(vks, vec![81]);
            }
            _ => panic!("解析成了别的动词"),
        }

        // name / need 都是可省字段，旧界面按最小字段发也能用
        let r: Request =
            serde_json::from_str(r#"{"op":"add_custom_key","combo":"F9","vks":[120]}"#).unwrap();
        match r {
            Request::AddCustomKey { name, need, .. } => {
                assert!(name.is_empty());
                assert_eq!(need, 0);
            }
            _ => panic!("解析成了别的动词"),
        }

        let r: Request =
            serde_json::from_str(r#"{"op":"remove_custom_key","id":"x:ctrlaltq"}"#).unwrap();
        assert!(matches!(r, Request::RemoveCustomKey { .. }));

        let r: Request =
            serde_json::from_str(r#"{"op":"rename_profile","from":"游戏","to":"打游戏"}"#).unwrap();
        assert!(matches!(r, Request::RenameProfile { .. }));

        let r: Request =
            serde_json::from_str(r#"{"op":"dup_profile","from":"游戏","to":"游戏 2"}"#).unwrap();
        assert!(matches!(r, Request::DupProfile { .. }));
    }

    #[test]
    fn err_response_shape() {
        let j = serde_json::to_string(&Response::err("组合为空")).unwrap();
        assert_eq!(j, r#"{"ok":false,"error":"组合为空"}"#);
    }
}
