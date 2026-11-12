//! KeyGuard Agent · P3 形态（双进程）
//!
//! 两种身份，一个二进制：
//!   kg-agent --agent        常驻服务端（High）：钩子 + 全屏检测 + 命名管道
//!   kg-agent lock|unlock|status|get-rules|put-rules|panic-release   客户端（Medium 界面将来走的就是这几个动词）
//!
//! 开发期的调试开关：
//!   kg-agent --auto              自动模式：检测到全屏就锁，退出全屏就放
//!   kg-agent --armed             直接进入拦截（手动，不跟全屏联动）
//!   kg-agent --features +win,-prtsc   逐项开关（+ 开 / - 关，可逗号分隔）
//!   kg-agent --probe             装钩子、跑 1 秒、打印状态与耗时后退出
//!   kg-agent --watch 8           采样 8 秒打印被吞的键，然后退出
//!
//! 运行中按 F8 切换锁定/解除；--auto 下解除只作用于本次全屏会话。

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use kg_core::proto::{Request, Response, StatusPayload};
use kg_core::rules::RuleSet;
use kg_core::{fullscreen, hook, ipc, mode_name, DetectCfg, Shared};

mod autostart;
mod console;
mod icon;
mod single;
mod tray;

const HELP: &str = "\
KeyGuard Agent (P3)

  常驻服务
  kg-agent --agent                 以 Agent 身份常驻：钩子 + 前台检测 + 命名管道

  客户端（一次请求即退出）
  kg-agent lock                    立即锁定
  kg-agent unlock                  立即解除
  kg-agent status                  打印状态（JSON）
  kg-agent get-rules               打印当前规则集（JSON）
  kg-agent put-rules <file.json>   替换规则集
  kg-agent panic-release           本次会话内不再拦截

  调试
  --auto               自动模式：照规则命中锁定，规则不再命中即恢复（推荐）
  --armed              启动即拦截（手动，不看规则）
  --features SPEC      逐项开关，如 +win,-ctrl-space（默认 7 项开）
  --probe              自检：装钩子 → 1 秒 → 打印状态、耗时、前台信息 → 退出
  --watch N            采样 N 秒，打印被吞的键位分布
  --allow-injected     连模拟输入也参与判定（仅供自动化测试）
  --enter MS           进锁迟滞，默认 1200
  --release MS         出锁迟滞，默认 2000
  -h, --help           显示本帮助

八项能力：win / alt-tab / alt-f4 / ctrl-esc / alt-esc / prtsc / win-g / ctrl-space
规则判定：只看前台是哪个程序（窗口化同样拦截）；全屏由内置「全屏程序」规则负责
规则文件：%APPDATA%\\KeyGuard\\rules.json（不存在时只内置「全屏程序」一条）";

fn rules_path() -> PathBuf {
    let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(base).join("KeyGuard").join("rules.json")
}

fn rules_path_str() -> String {
    rules_path().display().to_string()
}

fn load_rules() -> RuleSet {
    let p = rules_path();
    match std::fs::read_to_string(&p) {
        // 记事本 / PowerShell 存盘会带 UTF-8 BOM，serde 会当成非法 JSON 开头
        Ok(text) => match serde_json::from_str::<RuleSet>(text.trim_start_matches('\u{feff}')) {
            Ok(mut rs) if rs.validate().is_ok() => {
                let before = rs.version;
                // v3：总开关改成"默认不启用"；v4：全屏拦截变成「场景」；
                // v6：规则只看前台程序，全屏只由内置「全屏程序」规则覆盖，
                // 顺带删掉早期内置的示例规则（浏览器 / CS2 / VALORANT / OBS）。
                if rs.migrate() {
                    let upgraded = rs.version > before;
                    if let Err(e) = save_rules(&rs) {
                        eprintln!("升级规则文件失败（{e}），本次仍按升级后的内容运行");
                    } else if upgraded {
                        println!(
                            "规则已升级到 v{}（已写回 {}）：规则按前台程序判定，窗口化同样拦截；全屏由内置「全屏程序」规则负责",
                            kg_core::rules::RULES_VERSION,
                            rules_path_str()
                        );
                    } else {
                        println!("已补齐内置「全屏程序」规则 / 全屏场景（{}）", rules_path_str());
                    }
                }
                rs
            }
            Ok(rs) => {
                eprintln!("规则文件校验失败（{}），改用默认规则", rs.validate().unwrap_err());
                RuleSet::default()
            }
            Err(e) => {
                eprintln!("规则文件无法解析（{e}），改用默认规则");
                RuleSet::default()
            }
        },
        Err(_) => RuleSet::default(),
    }
}

fn save_rules(rs: &RuleSet) -> Result<(), String> {
    let p = rules_path();
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("建目录失败: {e}"))?;
    }
    let text = serde_json::to_string_pretty(rs).map_err(|e| e.to_string())?;
    // 先写临时文件再改名：断电也不会留下半个 JSON
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("写临时文件失败: {e}"))?;
    std::fs::rename(&tmp, &p).map_err(|e| format!("替换规则文件失败: {e}"))
}

fn apply_features(st: &Shared, spec: &str) {
    for part in spec.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (on, name) = match part.as_bytes()[0] {
            b'+' => (true, &part[1..]),
            b'-' => (false, &part[1..]),
            _ => (true, part),
        };
        if !st.set_feature(name, on) {
            eprintln!("未知能力: {name}（可用：win alt-tab alt-f4 ctrl-esc alt-esc prtsc win-g ctrl-space）");
            std::process::exit(2);
        }
    }
}

/// 客户端动词（一次请求一个进程）。集中列在这里，参数表和解析顺序只有一个来源。
const CLIENT_VERBS: [&str; 15] = [
    "lock",
    "unlock",
    "status",
    "get-rules",
    "put-rules",
    "panic-release",
    "autostart",
    "hotkeys",
    "set-hotkeys",
    "export",
    "import",
    "backend",
    "blocking",
    "always",
    "profile",
];

/// CLI 用的目录打印：按分组列 id / 组合 / 状态，方便一次性看全。
fn print_catalog(v: &serde_json::Value) {
    let on: Vec<String> = v["on"]
        .as_array()
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let profile = v["profile"].as_str().unwrap_or("");
    let mut last = String::new();
    let entries = v["entries"].as_array().cloned().unwrap_or_default();
    for e in entries {
        let cat = e["cat"].as_str().unwrap_or("").to_string();
        if cat != last {
            println!("\n[{cat}]");
            last = cat.clone();
        }
        let id = e["id"].as_str().unwrap_or("");
        let how = e["how"].as_str().unwrap_or("");
        let mark = match how {
            "win" => "  ·  ",
            "never" => "  -  ",
            _ if on.iter().any(|x| x == id) => "[x]  ",
            _ => "[ ]  ",
        };
        println!(
            "{mark}{:<16} {:<22} {}",
            id,
            e["combo"].as_str().unwrap_or(""),
            e["name"].as_str().unwrap_or("")
        );
    }
    println!("\n生效 {} 项{}", on.len(), if profile.is_empty() { String::new() } else { format!("（场景：{profile}）") });
    if let Some(scenes) = v.get("scenes").and_then(|x| x.as_array()) {
        if !scenes.is_empty() {
            println!("\n[场景]");
            for s in scenes {
                let name = s["name"].as_str().unwrap_or("");
                let count = s["count"].as_u64().unwrap_or(0);
                let used: Vec<&str> = s["used_by"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|x| x.as_str()).collect())
                    .unwrap_or_default();
                println!(
                    "  {:<14} {:<3} 项{}",
                    name,
                    count,
                    if used.is_empty() { String::new() } else { format!("  ← {}", used.join(" / ")) }
                );
            }
        }
    }
}

/// 把目录 + 当前开关打包给界面。how 用字符串，界面不用再查表。
///
/// `entries` = 内置目录（`custom:false`）+ 用户自定义条目（`custom:true`），
/// 两者字段形状完全一致，界面可以用同一套渲染代码。
/// `enabled` = 这一条在**全局开关集合**（base）里，与 `on`（当前生效，可能是场景覆盖）区分开。
fn catalog_json(st: &Shared) -> serde_json::Value {
    let rs = st.rules_snapshot();
    let base = st.hotkeys_base();
    let custom_base = st.custom_base();
    let mut entries: Vec<serde_json::Value> = kg_core::catalog::ENTRIES
        .iter()
        .enumerate()
        .map(|(i, e)| {
            serde_json::json!({
                "id": e.id,
                "cat": e.cat.title(),
                "name": e.name,
                "combo": e.combo,
                "how": match e.how {
                    kg_core::How::Hook => "hook",
                    kg_core::How::WinCover => "win",
                    kg_core::How::Accessibility => "access",
                    kg_core::How::Never => "never",
                },
                "note": e.note,
                "default_on": e.default_on,
                "custom": false,
                "enabled": i < 64 && base & (1u64 << i) != 0,
            })
        })
        .collect();
    for (i, c) in rs.custom.iter().enumerate().take(64) {
        entries.push(serde_json::json!({
            "id": c.id,
            "cat": "自定义",
            "name": c.name,
            "combo": c.combo,
            "how": "hook",
            "note": "用户添加的快捷键",
            "default_on": false,
            "custom": true,
            "enabled": custom_base & (1u64 << i) != 0,
        }));
    }
    serde_json::json!({
        "entries": entries,
        "on": kg_core::rules::ids_with_custom(&rs, st.hotkeys_eff(), st.custom_eff()),
        "base": kg_core::rules::ids_with_custom(&rs, base, custom_base),
        "profile": st.current_profile(),
        "scenes": kg_core::scenes::brief(&rs),
    })
}

/// 无障碍那四条走 SystemParametersInfo：钩子够不着它们，只能改系统设置。
/// 每次开关变化都全量重放一遍——幂等，且能治"上次被别的工具改回去了"。
/// 把无障碍那几条同步到系统设置。
///
/// `only` 是"只处理这些位"的过滤器：用户点了哪一条就只碰哪一条。
/// 传 `None` 表示全量对账（启动时用一次，靠 `set_accessibility_hotkey` 内部的
/// "没变化就不写" 保证不会白广播）。
fn apply_accessibility(st: &Shared, only: Option<u64>) {
    let on = st.hotkeys_eff();
    for (i, e) in kg_core::catalog::ENTRIES.iter().enumerate() {
        if e.how != kg_core::How::Accessibility {
            continue;
        }
        let bit = 1u64 << i;
        if let Some(mask) = only {
            if mask & bit == 0 {
                continue;
            }
        }
        let want_block = i < 64 && (on & bit) != 0;
        let _ = kg_core::win::set_accessibility_hotkey(e.id, want_block);
    }
}

/// 把开关写回规则文件（规则/场景/快捷键同住一个文件，改一处要整份落盘）。
fn persist_hotkeys(st: &Shared) -> Result<(), String> {
    let mut rs = st.rules_snapshot();
    rs.hotkeys = kg_core::rules::ids_with_custom(&rs, st.hotkeys_base(), st.custom_base());
    // 内存里也要跟着改：否则界面读到的还是旧的 hotkeys，导出配置会缺这一块
    st.set_rules(rs.clone());
    save_rules(&rs)
}

fn status_payload(st: &Shared) -> StatusPayload {
    StatusPayload::from_status(&st.status(), st.foreground(), ipc::sddl_for_current_user())
}

/// 按 `rs.hotkeys` 重算两套掩码并落盘。
///
/// 自定义条目的位下标 = 它在 `custom` 里的位置，所以**删/加一条之后所有位都会平移**，
/// 必须整体重算，不能只在旧掩码上按位删。
fn resync_masks(st: &Shared, rs: RuleSet) -> Result<(), String> {
    let builtin = kg_core::rules::mask_of(&rs.hotkeys);
    let custom = kg_core::rules::custom_mask_of(&rs, &rs.hotkeys);
    let saved = save_rules(&rs);
    st.set_rules(rs);
    let before = st.hotkeys_base();
    st.set_hotkeys_masks(builtin, custom);
    apply_accessibility(st, Some(before ^ builtin));
    // 场景生效中：按新掩码重算一遍生效集合
    let cur = st.current_profile();
    if !cur.is_empty() {
        st.apply_profile(Some(&cur));
        apply_accessibility(st, None);
    }
    saved
}

/// ---- 客户端：一个动词一次往返 ----
fn client(verb: &str, rest: &[String]) -> i32 {
    let req = match verb {
        "lock" => Request::Lock,
        "unlock" => Request::Unlock,
        "status" => Request::Status,
        "get-rules" => Request::GetRules,
        "backend" => {
            let name = rest.first().cloned().unwrap_or_else(|| "hook".into());
            Request::SetBackend { name }
        }
        "blocking" => {
            // 不带参数 = 读；on/off = 设置
            let on = match rest.first().map(|s| s.as_str()) {
                None => None,
                Some("on") | Some("1") | Some("true") => Some(true),
                _ => Some(false),
            };
            match on {
                Some(on) => Request::SetBlocking { on },
                None => Request::Status,
            }
        }
        "always" => Request::SetAlways {
            on: rest.first().map(|v| v != "off" && v != "0").unwrap_or(true),
        },
        "profile" => {
            let name = rest.first().cloned();
            let name = name.filter(|n| n != "clear" && n != "none");
            Request::SetProfile { name }
        }
        "panic-release" => Request::PanicRelease,
        "autostart" => Request::Autostart {
            on: rest.first().map(|v| match v.as_str() {
                "on" => true,
                "off" => false,
                _ => true,
            }),
        },
        "put-rules" => {
            let Some(path) = rest.first() else {
                eprintln!("用法: kg-agent put-rules <file.json>");
                return 2;
            };
            let Ok(text) = std::fs::read_to_string(path) else {
                eprintln!("读不到文件: {path}");
                return 2;
            };
            let rs: RuleSet = match serde_json::from_str(&text) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("JSON 不合法: {e}");
                    return 2;
                }
            };
            if let Err(e) = rs.validate() {
                eprintln!("规则校验失败: {e}");
                return 2;
            }
            Request::PutRules { rules: rs }
        }
        "hotkeys" => Request::GetCatalog,
        "set-hotkeys" => {
            let Some(list) = rest.first() else {
                eprintln!("用法: kg-agent set-hotkeys <id,id,...>   （空串 = 全关）");
                return 2;
            };
            let ids: Vec<String> = list
                .split(',')
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect();
            for id in &ids {
                // `x:` 前缀是自定义条目，合法 id 只有服务端（读 rules.json）知道，
                // 这里只做形状放行；真正不存在的 id 在 mask 里自然映射不到位。
                if kg_core::catalog::find(id).is_none()
                    && !id.starts_with(kg_core::rules::CUSTOM_PREFIX)
                {
                    eprintln!("未知快捷键 id: {id}（用 kg-agent hotkeys 看全部）");
                    return 2;
                }
            }
            Request::SetHotkeys { ids }
        }
        "export" => Request::ExportRules,
        "import" => {
            let Some(path) = rest.first() else {
                eprintln!("用法: kg-agent import <file.json>");
                return 2;
            };
            let Ok(text) = std::fs::read_to_string(path) else {
                eprintln!("读不到文件: {path}");
                return 2;
            };
            Request::ImportRules { json: text }
        }
        _ => return 2,
    };

    match ipc::call(&req) {
        Ok(resp) if resp.ok => {
            match (&req, resp.data) {
                (Request::Status, Some(v)) => {
                    // 状态做人读输出，规则类保持 JSON（要给界面/脚本吃）
                    match serde_json::from_value::<StatusPayload>(v) {
                        Ok(p) => {
                            println!("mode      : {} ({})", p.mode_name, p.mode);
                            println!("armed     : {}", p.armed);
                            println!("agent     : pid={} elevated={}", p.agent_pid, p.elevated);
                            println!(
                                "前台      : {} · {} · 全屏={} rid={:?} uipi={}",
                                p.fg.exe, p.fg.class, p.fg.fullscreen, p.fg.rid, p.fg.blocked_by_uipi
                            );
                            println!("拦截/放行 : {} / {}", p.blocked, p.passed);
                            println!(
                                "总开关    : {}{}",
                                if p.blocking { "开" } else { "关（完全不拦）" },
                                if p.manual.is_empty() { String::new() } else { format!(" · 手动场景 {}", p.manual) }
                            );
                            println!(
                                "执行后端  : {}{}",
                                p.backend,
                                if p.scancode_map {
                                    "（注册表已写入映射表，重启系统后生效）"
                                } else {
                                    ""
                                }
                            );
                            println!("回调耗时  : p50={}µs p99={}µs ({} 样本)", p.p50_us, p.p99_us, p.hook_samples);
                            println!("管道 SDDL : {}", p.pipe_sddl);
                        }
                        Err(e) => println!("状态解析失败: {e}"),
                    }
                }
                (Request::GetCatalog | Request::SetHotkeys { .. }, Some(v)) => {
                    print_catalog(&v);
                }
                (Request::ExportRules, Some(v)) => {
                    println!("{}", v.get("json").and_then(|x| x.as_str()).unwrap_or_default());
                }
                (_, Some(v)) => println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default()),
                (_, None) => println!("ok"),
            }
            0
        }
        Ok(resp) => {
            eprintln!("失败: {}", resp.error.unwrap_or_else(|| "未知错误".into()));
            1
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // 免管道：把快捷键目录直接打成 JSON。界面里的演示数据与文档都吃这一份，
    // 免得手抄一份常量、改目录时忘同步。
    if args.first().map(String::as_str) == Some("catalog-json") {
        let st = Shared::new();
        let v = catalog_json(&st);
        println!("{}", serde_json::to_string(&v).unwrap_or_else(|_| "{}".into()));
        return;
    }

    // 卸载/排障用：把本程序改过的系统状态全部还原，然后退出
    if args.first().map(String::as_str) == Some("--restore") {
        let lines = kg_core::win::restore_system_state();
        if lines.is_empty() {
            println!("没有需要还原的系统状态");
        } else {
            for line in lines {
                println!("{line}");
            }
        }
        return;
    }

    // 子命令优先：客户端模式不装钩子、不建共享状态
    if let Some(verb) = args.first().map(String::as_str) {
        if matches!(
            verb,
            "lock" | "unlock" | "status" | "get-rules" | "put-rules" | "panic-release" | "autostart"
        ) {
            std::process::exit(client(verb, &args[1..]));
        }
    }

    let mut armed = false;
    let mut auto = false;
    let mut agent = false;
    let mut probe = false;
    let mut allow_injected = false;
    let mut watch: Option<u64> = None;
    let mut cfg = DetectCfg::default();

    let st = Arc::new(Shared::new());

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--agent" => agent = true,
            "--auto" => auto = true,
            "--armed" => armed = true,
            "--probe" => probe = true,
            "--allow-injected" => allow_injected = true,
            "--watch" => {
                i += 1;
                watch = Some(args.get(i).and_then(|s| s.parse().ok()).unwrap_or(5));
            }
            "--enter" => {
                i += 1;
                cfg.enter =
                    Duration::from_millis(args.get(i).and_then(|s| s.parse().ok()).unwrap_or(1200));
            }
            "--release" => {
                i += 1;
                cfg.release =
                    Duration::from_millis(args.get(i).and_then(|s| s.parse().ok()).unwrap_or(2000));
            }
            "--features" => {
                i += 1;
                match args.get(i) {
                    Some(spec) => apply_features(&st, spec),
                    None => {
                        eprintln!("--features 需要一个值");
                        std::process::exit(2);
                    }
                }
            }
            "-h" | "--help" => {
                println!("{HELP}");
                return;
            }
            // 客户端动词放在参数解析之后：它们要读规则文件、建 Shared
            other if CLIENT_VERBS.contains(&other) => {
                let rest: Vec<String> = args[i + 1..].to_vec();
                std::process::exit(client(other, &rest));
            }
            other => {
                eprintln!("未知参数: {other}\n\n{HELP}");
                std::process::exit(2);
            }
        }
        i += 1;
    }

    if allow_injected {
        st.ignore_injected.store(false, Ordering::Relaxed);
    }

    // 规则：Agent 模式一定加载；调试模式也加载，保证「规则页改的东西」和「实测」是同一份
    let rs = load_rules();
    // 启动时先套用上次保存的快捷键开关，再装钩子：钩子读的就是这张表。
    // v8 起出厂全关，空列表就是「一条都不开」，不再回退到 default_mask。
    let boot_mask = kg_core::rules::mask_of(&rs.hotkeys);
    let boot_custom = kg_core::rules::custom_mask_of(&rs, &rs.hotkeys);
    st.set_rules(rs.clone());
    st.set_hotkeys_masks(boot_mask, boot_custom);
    // 执行后端也要跟着配置走：设成 L2/关闭 时这次启动就不挂钩子
    let mut backend = rs.backend.clone();
    // L2 是"写一张注册表就完事"的后端，所以每次启动都对齐一次：
    //   - 配置是 L2  → 表不在就写（用户重装/还原过注册表也能自愈）
    //   - 配置不是 L2 → 表还在就删（别让用户"关了还在吞键"）
    // 总开关也要跟配置走：关掉时 L2 不写表、L1 不锁
    st.set_blocking(rs.blocking);
    st.set_always(rs.always);
    if backend == "scancode" {
        if rs.blocking {
            if let Err(e) = kg_core::win::set_scancode_map(true) {
                println!("扫描码映射: 写入失败（{e}），本次退回 L1 钩子");
                backend = "hook".to_string();
            }
        } else if kg_core::win::scancode_map_owned() {
            match kg_core::win::set_scancode_map(false) {
                Ok(()) => println!("扫描码映射: 总开关是关的，已还原成接管前的状态（重启系统后 Win 键恢复）"),
                Err(e) => println!("扫描码映射: 还原失败（{e}）"),
            }
        }
    } else if kg_core::win::scancode_map_owned() {
        // 只回滚我们确实写过的那张表：没有备份记录就一个字都不动
        match kg_core::win::set_scancode_map(false) {
            Ok(()) => println!("扫描码映射: 已还原（配置不是 L2）"),
            Err(e) => println!("扫描码映射: 还原失败（{e}）"),
        }
    }
    st.set_backend(&backend);

    let elevated = kg_core::win::is_elevated();
    let self_rid = kg_core::win::self_rid();
    println!("=== KeyGuard Agent {} ===", env!("CARGO_PKG_VERSION"));
    println!(
        "权限      : {}  完整性 RID=0x{:04X}",
        if elevated {
            "High（管理员）— 可拦管理员权限的游戏"
        } else {
            "Medium（普通）— 管理员权限的游戏拦不住，需计划任务提权"
        },
        self_rid.unwrap_or(0)
    );

    if backend == "hook" {
        match hook::install(st.clone()) {
            Ok(h) => {
                println!("钩子      : WH_KEYBOARD_LL 已挂载");
                // 句柄必须活到进程退出，否则线程被 drop 时消息循环也跟着没了
                std::mem::forget(h);
            }
            Err(e) => {
                eprintln!("钩子安装失败，Win32 错误码 = {e}");
                std::process::exit(1);
            }
        }
    } else {
        println!(
            "钩子      : 未挂载（执行后端 = {}）{}",
            backend,
            match backend.as_str() {
                "scancode" => "，由键盘驱动层吞键，重启系统后生效",
                _ => "，只做检测与提示，不改动任何按键",
            }
        );
    }

    let s = st.status();
    println!("能力掩码  : 0x{:02X}（{} 项开）", s.features, s.features.count_ones());
    println!(
        "规则      : {} 条（{}）",
        rs.rules.len(),
        rules_path().display()
    );

    // 手动锁定要在 probe/watch 之前生效，否则 --armed --watch 会测到「什么都没拦」
    if armed {
        st.set_mode(kg_core::MODE_LOCKED);
    }

    // ---- Agent 模式：全屏检测自动开 + 起管道服务端 ----
    if agent {
        // 先把控制台藏掉再干别的：下面那句管道探测最坏要等 2 秒（WaitNamedPipe），
        // 晚藏就是用户眼前亮两秒的黑窗口。--agent 是常驻服务，本来也不需要输出。
        single::hide_console();

        // 任务版和手动版不能同时存在：两条钩子会让每次按键被吞两遍
        if !single::acquire() {
            std::process::exit(0);
        }
        // 管道是本会话唯一的服务入口：能连通就说明已经有人在服务。
        // 这一条比互斥体强，跨版本也成立（老版本 Agent 没有互斥体）。
        if kg_core::ipc::call(&Request::Status).is_ok() {
            std::process::exit(0);
        }
        // 无障碍快捷键只有 Agent 在跑时才接管：CLI 查个状态不该改系统设置
        apply_accessibility(&st, None);
        std::mem::forget(fullscreen::spawn(st.clone(), DetectCfg { auto: true, ..cfg }));

        let handler_st = st.clone();
        let handler: ipc::Handler = Arc::new(move |req: Request| -> Response {
            let h = &handler_st;
            match req {
                Request::Lock => {
                    h.set_hold(true);
                    h.set_mode(kg_core::MODE_LOCKED);
                    Response::ok()
                }
                Request::Unlock => {
                    h.set_hold(false);
                    h.set_mode(kg_core::MODE_IDLE);
                    Response::ok()
                }
                Request::PanicRelease => {
                    tray::release_now(h);
                    Response::ok()
                }
                Request::Autostart { on } => {
                    let result = match on {
                        Some(true) => autostart::enable(),
                        Some(false) => autostart::disable(),
                        None => Ok(()),
                    };
                    match result {
                        Ok(()) => Response::with(serde_json::json!({ "enabled": autostart::is_enabled() })),
                        Err(e) => Response::err(e),
                    }
                }
                Request::SetFeatures { mask } => {
                    h.set_feature_mask(mask);
                    Response::with(status_payload(h))
                }
                Request::GetCatalog => Response::with(catalog_json(h)),
                Request::SetHotkeys { ids } => {
                    // ids 是「内置 + 自定义」的混合列表，按各自的编号空间拆成两套掩码
                    let mask = kg_core::rules::mask_of(&ids);
                    let custom = {
                        let rs = h.rules_snapshot();
                        kg_core::rules::custom_mask_of(&rs, &ids)
                    };
                    let before = h.hotkeys_base();
                    h.set_hotkeys_masks(mask, custom);
                    // 只同步真正被改动的那几条：每写一次系统设置都会广播，
                    // 广播多了 explorer 会重读主题（壁纸跟着变）
                    apply_accessibility(h, Some(before ^ mask));
                    if let Err(e) = persist_hotkeys(h) {
                        return Response::err(e);
                    }
                    Response::with(catalog_json(h))
                }
                Request::ExportRules => Response::with(serde_json::json!({
                    "json": serde_json::to_string_pretty(&kg_core::scenes::bundle(&h.rules_snapshot()))
                        .unwrap_or_default(),
                    "path": rules_path().display().to_string(),
                })),
                Request::ImportRules { json } => {
                    let rs: RuleSet = match kg_core::scenes::unbundle(&json) {
                        Ok(v) => v,
                        Err(e) => return Response::err(e),
                    };
                    if let Err(e) = rs.validate() {
                        return Response::err(e);
                    }
                    if let Err(e) = save_rules(&rs) {
                        return Response::err(e);
                    }
                    // 快捷键跟着一起换：导入的是"一份完整配置"，不是只换规则
                    let mask = kg_core::rules::mask_of(&rs.hotkeys);
                    let custom = kg_core::rules::custom_mask_of(&rs, &rs.hotkeys);
                    let before = h.hotkeys_base();
                    h.set_rules(rs);
                    h.set_hotkeys_masks(mask, custom);
                    apply_accessibility(h, Some(before ^ mask));
                    Response::with(serde_json::json!({ "ok": true }))
                }
                Request::SetScene { name, ids, trigger } => {
                    let mut rs = h.rules_snapshot();
                    if let Err(e) = rs.upsert_profile(&name, &ids, trigger.as_deref()) {
                        return Response::err(e);
                    }
                    if let Err(e) = rs.validate() {
                        return Response::err(e);
                    }
                    if let Err(e) = save_rules(&rs) {
                        return Response::err(e);
                    }
                    let applied = h.current_profile() == name;
                    h.set_rules(rs);
                    if applied {
                        h.apply_profile(Some(&name));
                        apply_accessibility(h, None);
                    }
                    Response::with(serde_json::json!({ "ok": true, "applied": applied }))
                }
                Request::DeleteScene { name } => {
                    let mut rs = h.rules_snapshot();
                    if !rs.remove_profile(&name) {
                        return Response::err(format!("没有这个场景: {name}"));
                    }
                    if let Err(e) = save_rules(&rs) {
                        return Response::err(e);
                    }
                    let was = h.current_profile();
                    h.set_rules(rs);
                    if !was.is_empty() {
                        h.apply_profile(None);
                        apply_accessibility(h, None);
                    }
                    Response::with(serde_json::json!({ "ok": true }))
                }
                Request::SetBackend { name } => {
                    if !matches!(name.as_str(), "hook" | "scancode" | "off") {
                        return Response::err(format!("未知的执行后端: {name}"));
                    }
                    let prev = h.backend();
                    // L2 要落到注册表；从 L2 切走再把它删掉，否则用户会一直带着一张映射表
                    let applied = if name == "scancode" {
                        kg_core::win::set_scancode_map(true).map(|_| "已写入扫描码映射，重启系统后生效")
                    } else if prev == "scancode" {
                        kg_core::win::set_scancode_map(false).map(|_| "已删除扫描码映射，重启系统后恢复")
                    } else {
                        Ok("已保存，重启 Agent 后生效")
                    };
                    let note = match applied {
                        Ok(n) => n,
                        Err(e) => return Response::err(e),
                    };
                    let mut rs = h.rules_snapshot();
                    rs.backend = name.clone();
                    if let Err(e) = save_rules(&rs) {
                        return Response::err(e);
                    }
                    h.set_rules(rs);
                    // 注意：不调用 set_backend —— 内存里的 backend 表示"这个进程实际在用哪个后端"，
                    // 改了配置要等重启才换实现，否则界面会显示一个假的"当前"。
                    Response::with(serde_json::json!({
                        "backend": name,
                        "previous": prev,
                        "note": note,
                        "reboot": name == "scancode" || prev == "scancode",
                        "restart": true,
                    }))
                }
                Request::SetBlocking { on } => {
                    h.set_blocking(on);
                    // L2 是系统级静态映射，只能靠改注册表"停用"，而且重启才生效。
                    // 这里如实说明，别让用户以为按一下就不吞键了。
                    let note: String;
                    let mut reboot = false;
                    if h.backend() == "scancode" {
                        match kg_core::win::set_scancode_map(on) {
                            Ok(()) => {
                                reboot = true;
                                note = if on {
                                    "映射表已写入，重启系统后 Win 键开始被屏蔽".to_string()
                                } else {
                                    "映射表已删除，重启系统后 Win 键恢复".to_string()
                                };
                            }
                            Err(e) => return Response::err(e),
                        }
                    } else if !on {
                        note = "已停用，不再拦截任何按键".to_string();
                    } else {
                        note = "已启用".to_string();
                    }
                    let mut rs = h.rules_snapshot();
                    rs.blocking = on;
                    if let Err(e) = save_rules(&rs) {
                        return Response::err(e);
                    }
                    h.set_rules(rs);
                    Response::with(serde_json::json!({
                        "blocking": on,
                        "note": note,
                        "reboot": reboot,
                    }))
                }
                Request::SetAlways { on } => {
                    h.set_always(on);
                    // 打开就立刻锁（不用等全屏），关掉立刻解开
                    h.set_mode(if on {
                        kg_core::state::MODE_LOCKED
                    } else {
                        kg_core::state::MODE_IDLE
                    });
                    let mut rs = h.rules_snapshot();
                    rs.always = on;
                    if let Err(e) = save_rules(&rs) {
                        return Response::err(e);
                    }
                    h.set_rules(rs);
                    Response::with(serde_json::json!({ "always": on }))
                }
                Request::SetProfile { name } => {
                    let want = name.unwrap_or_default();
                    let want = want.trim();
                    if !want.is_empty() {
                        let rs = h.rules_snapshot();
                        if !rs.profiles.iter().any(|p| p.name == want || p.id == want) {
                            return Response::err(format!("没有这个场景: {want}"));
                        }
                    }
                    h.set_manual_profile(if want.is_empty() { None } else { Some(want) });
                    // 立刻按新场景换一套键表（锁定中也要马上换）
                    h.apply_profile(if want.is_empty() { None } else { Some(want) });
                    Response::with(serde_json::json!({
                        "manual": want,
                        "profile": h.current_profile(),
                    }))
                }
                Request::ListProcs => {
                    // 上限 400 条：系统进程上千个，界面下拉框也用不了那么多
                    Response::with(serde_json::json!({ "procs": kg_core::win::list_procs(400) }))
                }
                Request::AddCustomKey { name, combo, need, vks } => {
                    let mut rs = h.rules_snapshot();
                    let id = match rs.add_custom(&name, &combo, need, &vks) {
                        Ok(v) => v,
                        Err(e) => return Response::err(e),
                    };
                    if let Err(e) = rs.validate() {
                        return Response::err(e);
                    }
                    if let Err(e) = resync_masks(h, rs) {
                        return Response::err(e);
                    }
                    Response::with(serde_json::json!({ "id": id }))
                }
                Request::RemoveCustomKey { id } => {
                    let mut rs = h.rules_snapshot();
                    if !rs.remove_custom(&id) {
                        return Response::err(format!("没有这条自定义快捷键: {id}"));
                    }
                    if let Err(e) = resync_masks(h, rs) {
                        return Response::err(e);
                    }
                    Response::with(serde_json::json!({ "ok": true, "removed": id }))
                }
                Request::RenameProfile { from, to } => {
                    let mut rs = h.rules_snapshot();
                    let cur = h.current_profile();
                    let manual = h.manual_profile();
                    // 改名后 id 会按 slug 重算，旧名字/旧 id 都得留着判断「当前在不在这个场景上」
                    let old_id = rs.profile_by(&from).map(|p| p.id.clone()).unwrap_or_default();
                    if let Err(e) = rs.rename_profile(&from, &to) {
                        return Response::err(e);
                    }
                    if let Err(e) = rs.validate() {
                        return Response::err(e);
                    }
                    let new_id = rs.profile_by(&to).map(|p| p.id.clone()).unwrap_or_default();
                    if let Err(e) = save_rules(&rs) {
                        return Response::err(e);
                    }
                    h.set_rules(rs);
                    // 正在套用的就是这个场景：立刻切到新名字，否则会掉回全局
                    if !cur.is_empty() && (cur == from || cur == old_id) {
                        h.apply_profile(Some(&to));
                        apply_accessibility(h, None);
                    }
                    if manual == from || manual == old_id {
                        h.set_manual_profile(Some(&to));
                    }
                    Response::with(serde_json::json!({ "ok": true, "id": new_id }))
                }
                Request::DupProfile { from, to } => {
                    let mut rs = h.rules_snapshot();
                    if let Err(e) = rs.dup_profile(&from, &to) {
                        return Response::err(e);
                    }
                    if let Err(e) = rs.validate() {
                        return Response::err(e);
                    }
                    let new_id = rs.profile_by(&to).map(|p| p.id.clone()).unwrap_or_default();
                    if let Err(e) = save_rules(&rs) {
                        return Response::err(e);
                    }
                    h.set_rules(rs);
                    Response::with(serde_json::json!({ "ok": true, "id": new_id }))
                }
                Request::Status => Response::with(status_payload(h)),
                Request::GetRules => Response::with(h.rules_snapshot()),
                Request::PutRules { rules } => {
                    if let Err(e) = rules.validate() {
                        return Response::err(e);
                    }
                    if let Err(e) = save_rules(&rules) {
                        return Response::err(e);
                    }
                    h.set_rules(rules);
                    Response::ok()
                }
            }
        });

        std::thread::spawn(move || match ipc::serve(handler) {
            Ok(()) => eprintln!("管道服务端意外退出"),
            Err(5) => {
                // 已有别的 Agent 占着管道：本进程绝不能继续（会变成第二套钩子）
                eprintln!("管道已被占用，本进程退出。");
                std::process::exit(0);
            }
            Err(e) => eprintln!("管道服务端启动失败，Win32 = {e}"),
        });

        // 托盘 + 全局解除热键：放在管道之后，保证界面先能连上
        let ui = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("kg-ui.exe")))
            .filter(|p| p.exists());
        match tray::spawn(st.clone(), ui) {
            Ok(()) => {}
            Err(e) => eprintln!("托盘线程启动失败：{e}"),
        }

        println!("管道      : {}", ipc::pipe_name());
        println!("SDDL      : {}", ipc::sddl_for_current_user());
        println!("\nAgent 就绪。Ctrl+C 退出。\n");

        let mut last_mode = st.current_mode();
        let mut last_blocked = 0u64;
        loop {
            std::thread::sleep(Duration::from_secs(2));
            let m = st.current_mode();
            if m != last_mode {
                println!(">>> {}：{}", mode_name(m), describe(st.as_ref(), m));
                last_mode = m;
            }
            let s = st.status();
            println!(
                "mode={:<13} armed={:<5} blocked={:<6} Δ={:<4} fg={}",
                mode_name(s.mode),
                s.armed,
                s.blocked,
                s.blocked.saturating_sub(last_blocked),
                empty(&s.fg_exe)
            );
            last_blocked = s.blocked;
        }
    }

    // 自检线程：只观察不动作时也不会误锁（auto=false 时 mode 恒为 IDLE）
    let detect_cfg = DetectCfg { auto, ..cfg };
    if auto || probe || watch.is_some() {
        std::mem::forget(fullscreen::spawn(st.clone(), DetectCfg { auto, ..cfg }));
        println!(
            "全屏检测  : {}（迟滞 {}/{} ms，采样 {} ms）",
            if auto { "自动锁定" } else { "仅观察" },
            detect_cfg.enter.as_millis(),
            detect_cfg.release.as_millis(),
            detect_cfg.poll.as_millis()
        );
    }

    if probe {
        std::thread::sleep(Duration::from_millis(1000));
        let s = st.status();
        let fg = st.foreground();
        println!(
            "自检      : mode={} armed={} blocked={} passed={}",
            mode_name(s.mode),
            s.armed,
            s.blocked,
            s.passed
        );
        println!(
            "前台      : pid={} exe={} class={} 全屏={} cloaked={} rid=0x{:04X} uipi={}",
            fg.pid,
            empty(&fg.exe),
            empty(&fg.class),
            fg.fullscreen,
            fg.cloaked,
            fg.rid.unwrap_or(0),
            fg.blocked_by_uipi
        );
        println!(
            "回调耗时  : p50={}µs p99={}µs 样本={}",
            s.p50_us, s.p99_us, s.hook_samples
        );
        println!("自检结果  : OK（钩子线程活着，全屏检测线程已启动）");
        return;
    }

    if let Some(secs) = watch {
        println!("采样 {secs}s ...");
        let mut last = String::new();
        for _ in 0..secs * 2 {
            std::thread::sleep(Duration::from_millis(500));
            let fg = st.foreground();
            let line = format!(
                "mode={} armed={} exe={} class={} 全屏={} covers={} cloaked={} quns={}",
                mode_name(st.current_mode()),
                st.is_armed(),
                empty(&fg.exe),
                empty(&fg.class),
                fg.fullscreen,
                fg.covers,
                fg.cloaked,
                fg.quns
            );
            if line != last {
                println!("{line}");
                last = line;
            }
        }
        let mut rows: Vec<(u32, u64)> = (0..256u32)
            .map(|v| (v, st.vk_hits(v)))
            .filter(|(_, n)| *n > 0)
            .collect();
        rows.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
        println!("--- 被吞的键 ---");
        for (v, n) in rows {
            println!("vk=0x{v:02X} count={n}");
        }
        let (p50, p99, n) = st.hist_percentiles();
        println!("回调耗时: p50={p50}µs p99={p99}µs 样本={n}");
        print_hist(&st);
        return;
    }

    println!("按 F8 切换锁定，Ctrl+C 退出。\n");

    let mut last_blocked = 0u64;
    let mut last_mode = st.current_mode();
    loop {
        std::thread::sleep(Duration::from_secs(1));

        let m = st.current_mode();
        if m != last_mode {
            println!(">>> {}：{}", mode_name(m), describe(st.as_ref(), m));
            last_mode = m;
        }
        st.signal.store(0, Ordering::Relaxed);

        let s = st.status();
        println!(
            "mode={:<8} armed={:<5} blocked={:<6} Δ={:<4} p99={}µs fg={}",
            mode_name(s.mode),
            s.armed,
            s.blocked,
            s.blocked.saturating_sub(last_blocked),
            s.p99_us,
            empty(&s.fg_exe)
        );
        last_blocked = s.blocked;
    }
}

fn describe(st: &Shared, m: u8) -> String {
    match m {
        kg_core::MODE_LOCKED => {
            "Win / Alt+Tab / Alt+F4 / Ctrl+Esc / Alt+Esc / PrtSc 全部拦截".to_string()
        }
        kg_core::MODE_DEGRADED => {
            let fg = st.foreground();
            format!(
                "前台 {} 的权限高于本进程，钩子回调不会被触发；请以管理员身份重启 Agent",
                empty(&fg.exe)
            )
        }
        kg_core::MODE_TEMP_UNLOCK => "本次全屏会话不再拦截，退出游戏后自动恢复".to_string(),
        _ => "所有按键原样放行".to_string(),
    }
}

fn print_hist(st: &Shared) {
    let mut rows = Vec::new();
    for us in 0..kg_core::state::HIST_BUCKETS {
        let c = st.hist[us].load(Ordering::Relaxed);
        if c > 0 {
            rows.push((us, c));
        }
    }
    let over = st.hist_over.load(Ordering::Relaxed);
    if rows.is_empty() && over == 0 {
        println!("直方图  : 空（没有测到任何按键）");
        return;
    }
    print!("直方图  : ");
    for (us, c) in rows.iter().take(12) {
        print!("{us}µs×{c}  ");
    }
    if over > 0 {
        print!("≥{}µs×{over}", kg_core::state::HIST_OVER_US);
    }
    println!();
}

fn empty(s: &str) -> &str {
    if s.is_empty() {
        "-"
    } else {
        s
    }
}
