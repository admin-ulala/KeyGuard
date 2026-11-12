use crate::{Action, Rule, RuleSet};
use serde::{Deserialize, Serialize};

/// 场景增删改：id 用 name 归一化（小写去空格），界面上只暴露 name。
impl RuleSet {
    /// trigger 为空则沿用已有场景的触发条件（新场景默认按进程命中触发）
    pub fn upsert_profile(&mut self, name: &str, ids: &[String], trigger: Option<&str>) -> Result<&str, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("场景名不能为空".into());
        }
        let id = slug(name);
        let on: Vec<String> = ids
            .iter()
            // 自定义快捷键不在 catalog 里，别被过滤掉（否则场景永远存不住它们）
            .filter(|i| crate::catalog::find(i).is_some() || self.custom_index(i).is_some())
            .cloned()
            .collect();
        let trig = trigger.unwrap_or("").trim().to_string();
        if !trig.is_empty() && !crate::rules::TRIGGERS.contains(&trig.as_str()) {
            return Err(format!("未知的触发条件: {trig}"));
        }
        if let Some(p) = self.profiles.iter_mut().find(|p| p.id == id) {
            p.name = name.to_string();
            p.on = on;
            if !trig.is_empty() {
                p.trigger = trig;
            }
        } else {
            self.profiles.push(crate::Profile {
                id: id.clone(),
                name: name.to_string(),
                trigger: if trig.is_empty() { "process".to_string() } else { trig },
                enabled: true,
                on,
            });
        }
        Ok("ok")
    }

    pub fn remove_profile(&mut self, name_or_id: &str) -> bool {
        let id = slug(name_or_id);
        let before = self.profiles.len();
        self.profiles
            .retain(|p| p.id != id && p.name != name_or_id);
        if self.profiles.len() == before {
            return false;
        }
        // 引用它的规则退回"不切场景"，否则会指向一个不存在的场景
        for r in self.rules.iter_mut() {
            if slug(&r.profile) == id || r.profile == name_or_id {
                r.profile.clear();
            }
        }
        true
    }

    pub fn profile_by(&self, name_or_id: &str) -> Option<&crate::Profile> {
        let id = slug(name_or_id);
        self.profiles
            .iter()
            .find(|p| p.id == id || p.name == name_or_id)
    }

    /// 场景改名：id 按新名字的 slug 重算，**引用它的规则一起改写成新名字**。
    ///
    /// 规则里存的是场景名（见 `Rule::profile`），不改就会指向一个不存在的场景，
    /// 表现为「改名之后那条规则突然不套场景了」。
    pub fn rename_profile(&mut self, from: &str, to: &str) -> Result<(), String> {
        let to = to.trim();
        if to.is_empty() {
            return Err("场景名不能为空".into());
        }
        let from_id = slug(from);
        let idx = self
            .profiles
            .iter()
            .position(|p| p.id == from_id || p.name == from)
            .ok_or_else(|| format!("没有这个场景: {from}"))?;
        let new_id = slug(to);
        if self
            .profiles
            .iter()
            .enumerate()
            .any(|(i, p)| i != idx && (p.id == new_id || p.name == to))
        {
            return Err(format!("场景名已被占用: {to}"));
        }
        let old_id = self.profiles[idx].id.clone();
        let old_name = self.profiles[idx].name.clone();
        self.profiles[idx].id = new_id;
        self.profiles[idx].name = to.to_string();
        for r in self.rules.iter_mut() {
            if r.profile == old_name || r.profile == old_id || slug(&r.profile) == old_id {
                r.profile = to.to_string();
            }
        }
        Ok(())
    }

    /// 复制一份场景（开关集合一起复制，触发条件沿用源场景）。
    pub fn dup_profile(&mut self, from: &str, to: &str) -> Result<(), String> {
        let to = to.trim();
        if to.is_empty() {
            return Err("场景名不能为空".into());
        }
        let src = self
            .profile_by(from)
            .cloned()
            .ok_or_else(|| format!("没有这个场景: {from}"))?;
        let new_id = slug(to);
        if self.profiles.iter().any(|p| p.id == new_id || p.name == to) {
            return Err(format!("场景名已被占用: {to}"));
        }
        self.profiles.push(crate::Profile {
            id: new_id,
            name: to.to_string(),
            ..src
        });
        Ok(())
    }
}

/// 名称 → 稳定 id：小写、去掉空白与常见分隔符，保留中英文。
fn slug(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_whitespace() && *c != '-' && *c != '_')
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// 场景变更后要落盘的 JSON（导出用）。
#[derive(Serialize, Deserialize)]
pub struct Bundle {
    pub app: String,
    pub kind: String,
    pub data: RuleSet,
}

pub fn bundle(rs: &RuleSet) -> Bundle {
    Bundle {
        app: "KeyGuard".into(),
        kind: "rules".into(),
        data: rs.clone(),
    }
}

pub fn unbundle(json: &str) -> Result<RuleSet, String> {
    // 两种都收：完整包（含 app/kind）与裸 RuleSet
    if let Ok(b) = serde_json::from_str::<Bundle>(json) {
        if b.app == "KeyGuard" {
            return Ok(b.data);
        }
    }
    serde_json::from_str::<RuleSet>(json).map_err(|e| format!("不是 KeyGuard 规则文件: {e}"))
}

/// 给界面用的场景摘要。
#[derive(Serialize)]
pub struct ProfileBrief {
    pub id: String,
    pub name: String,
    pub on: Vec<String>,
    pub count: usize,
    pub used_by: Vec<String>,
}

pub fn brief(rs: &RuleSet) -> Vec<ProfileBrief> {
    rs.profiles
        .iter()
        .map(|p| ProfileBrief {
            id: p.id.clone(),
            name: p.name.clone(),
            on: p.on.clone(),
            count: p.on.len(),
            used_by: rs
                .rules
                .iter()
                .filter(|r| !r.profile.is_empty() && slug(&r.profile) == p.id)
                .map(|r| r.name.clone())
                .collect(),
        })
        .collect()
}

#[allow(dead_code)]
fn _assert_types(_: Action, _: Rule) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn rs_with_profile() -> RuleSet {
        let mut rs = RuleSet::default();
        rs.add_custom("Q", "Ctrl + Alt + Q", 3, &[0x51]).unwrap();
        rs.upsert_profile("游戏", &["win".into(), "x:ctrlaltq".into()], Some("process"))
            .unwrap();
        rs.rules.push(Rule {
            id: "mine".into(),
            name: "我的游戏".into(),
            exe: Vec::new(),
            class: String::new(),
            fullscreen: false,
            always: false,
            foreground: true,
            action: Action::Lock,
            delay_ms: 0,
            release_delay_ms: 0,
            ttl_ms: 0,
            profile: "游戏".into(),
            enabled: true,
        });
        rs
    }

    #[test]
    fn upsert_keeps_custom_ids() {
        let rs = rs_with_profile();
        assert_eq!(rs.profiles[0].on, vec!["win", "x:ctrlaltq"], "自定义 id 不能被过滤掉");
    }

    #[test]
    fn rename_profile_rewrites_rule_reference() {
        let mut rs = rs_with_profile();
        rs.rename_profile("游戏", "打游戏").unwrap();
        assert_eq!(rs.profiles[0].id, "打游戏");
        assert_eq!(rs.profiles[0].name, "打游戏");
        assert_eq!(rs.rules[0].profile, "打游戏", "规则里的引用必须跟着改");
        assert!(rs.rename_profile("不存在", "x").is_err());
        assert!(rs.rename_profile("打游戏", "  ").is_err());
    }

    #[test]
    fn rename_rejects_duplicate_name() {
        let mut rs = rs_with_profile();
        rs.upsert_profile("挂机", &[], None).unwrap();
        assert!(rs.rename_profile("游戏", "挂机").is_err(), "撞名要拒绝");
    }

    #[test]
    fn dup_profile_copies_switches() {
        let mut rs = rs_with_profile();
        rs.dup_profile("游戏", "游戏 2").unwrap();
        assert_eq!(rs.profiles.len(), 2);
        let p = &rs.profiles[1];
        assert_eq!(p.id, "游戏2");
        assert_eq!(p.name, "游戏 2");
        assert_eq!(p.on, vec!["win", "x:ctrlaltq"]);
        assert_eq!(p.trigger, "process");
        assert!(rs.dup_profile("游戏", "游戏 2").is_err(), "撞名要拒绝");
        assert!(rs.dup_profile("没有", "x").is_err());
    }
}
