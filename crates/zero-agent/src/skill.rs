use crate::tool::Tool;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

/// Skill 是可复用的能力单元：向 system prompt 注入指令，并携带若干内置工具。
/// 执行能力由工具本身提供，Skill 本身不实现任何执行逻辑。
pub struct Skill {
    pub name: String,
    pub description: String,
    /// 注入 system prompt 的指令段落
    pub instructions: Vec<String>,
    /// 该 skill 携带的内置工具
    pub tools: Vec<Arc<dyn Tool>>,
}

impl Skill {
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Skill {
            name: name.into(),
            description: description.into(),
            instructions: Vec::new(),
            tools: Vec::new(),
        }
    }

    pub fn with_instruction(mut self, instruction: impl Into<String>) -> Self {
        self.instructions.push(instruction.into());
        self
    }

    pub fn with_tool(mut self, tool: Arc<dyn Tool>) -> Self {
        self.tools.push(tool);
        self
    }

    /// 生成父 agent system prompt 片段：只包含 name + description。
    /// 让 LLM 知道有哪些 skill 可通过 spawn_subagent 调用，不污染上下文。
    pub fn to_summary_section(&self) -> String {
        format!("- {} : {}", self.name, self.description)
    }

    /// 生成子 agent system prompt 片段：包含完整 instructions。
    /// 只在 skill 被选中、子 agent 实际执行时才注入。
    pub fn to_instructions_section(&self) -> String {
        if self.instructions.is_empty() {
            return String::new();
        }
        let mut lines = vec![format!("## Skill: {}", self.name)];
        for inst in &self.instructions {
            lines.push(format!("- {}", inst));
        }
        format!("\n\n{}\n", lines.join("\n"))
    }
}

// ---------------------------------------------------------------------------
// SkillRegistry
// ---------------------------------------------------------------------------

/// Skill 注册表。用 `Arc<SkillRegistry>` 在父子 agent 间共享，
/// 子 agent 通过 skill 名字列表从中取出所需 skill 独立构建自己的 context。
#[derive(Clone, Default)]
pub struct SkillRegistry {
    pub(crate) skills: HashMap<String, Arc<Skill>>,
}

impl SkillRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册一个 skill（owned）
    pub fn register(&mut self, skill: Skill) -> &mut Self {
        self.skills.insert(skill.name.clone(), Arc::new(skill));
        self
    }

    /// 注册一个已经 Arc 包装的 skill
    pub fn register_arc(&mut self, skill: Arc<Skill>) -> &mut Self {
        self.skills.insert(skill.name.clone(), skill);
        self
    }

    /// 按名字查找 skill
    pub fn get(&self, name: &str) -> Option<Arc<Skill>> {
        self.skills.get(name).cloned()
    }

    /// 按名字列表批量取出 skill（忽略不存在的名字，顺序保留）
    pub fn get_many(&self, names: &[String]) -> Vec<Arc<Skill>> {
        names.iter().filter_map(|n| self.get(n)).collect()
    }

    /// 列出所有已注册的 skill 名
    pub fn names(&self) -> Vec<&str> {
        self.skills.keys().map(String::as_str).collect()
    }
}

// ---------------------------------------------------------------------------
// Markdown 加载
// ---------------------------------------------------------------------------

/// 从 markdown 文件解析出的中间结构，包含 allowed_tools 名称列表。
/// 调用方拿到后，按名字查找工具，再调用 `.with_tool()` 绑定。
pub struct SkillDef {
    pub skill: Skill,
    /// frontmatter 中声明的工具名列表，调用方负责解析绑定
    pub allowed_tools: Vec<String>,
}

#[derive(Debug, serde::Deserialize)]
struct SkillFrontmatter {
    name: Option<String>,
    description: Option<String>,
    #[serde(rename = "allowed-tools", default)]
    allowed_tools: Vec<String>,
}

fn parse_frontmatter(content: &str) -> (Option<String>, String) {
    let content = content.replace("\r\n", "\n").replace('\r', "\n");
    if !content.starts_with("---") {
        return (None, content);
    }
    match content[3..].find("\n---") {
        None => (None, content),
        Some(end) => {
            let yaml = content[4..end + 3].to_string();
            let body = content[end + 7..].trim().to_string();
            (Some(yaml), body)
        }
    }
}

fn validate_name(name: &str) -> Result<(), String> {
    if name.len() > 64 {
        return Err(format!("name exceeds 64 characters ({})", name.len()));
    }
    if !name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_') {
        return Err("name must contain only lowercase letters, digits, hyphens, and underscores".to_string());
    }
    if name.starts_with('-') || name.ends_with('-') {
        return Err("name must not start or end with a hyphen".to_string());
    }
    if name.contains("--") {
        return Err("name must not contain consecutive hyphens".to_string());
    }
    Ok(())
}

impl Skill {
    /// 从 markdown 文件加载，返回 `SkillDef`。
    ///
    /// 格式：
    /// ```markdown
    /// ---
    /// name: my-skill           # 可选，默认用文件名（去掉 .md）
    /// description: 一句话描述   # 必填
    /// allowed-tools:           # 可选，声明需要绑定的工具名
    ///   - calculator
    ///   - json_validate
    /// ---
    ///
    /// markdown body 作为 instructions 注入 system prompt。
    /// ```
    ///
    /// 调用方拿到 `SkillDef` 后，根据 `allowed_tools` 按名字查找工具，
    /// 再调用 `skill_def.skill.with_tool(...)` 绑定。
    /// 从单个 markdown 文件加载，默认 name 取文件名（去掉 .md）。
    pub fn from_file(path: impl AsRef<Path>) -> Result<SkillDef, String> {
        let path = path.as_ref();
        let default_name = path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unnamed")
            .to_string();
        Skill::from_file_with_default_name(path, &default_name)
    }

    /// 从 markdown 文件加载，使用指定的默认 name（供 load_dir 用目录名覆盖）。
    pub fn from_file_with_default_name(path: impl AsRef<Path>, default_name: &str) -> Result<SkillDef, String> {
        let path = path.as_ref();
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
        skill_from_content(&content, default_name, path)
    }

    /// 从 skills 根目录加载所有 skill。
    ///
    /// 目录结构约定：
    /// ```
    /// skills/
    ///   math-assistant/
    ///     SKILL.md        ← 每个子目录包含一个 SKILL.md
    ///   greeting-expert/
    ///     SKILL.md
    /// ```
    ///
    /// 子目录名作为 skill 的默认 name（frontmatter 的 `name` 字段可覆盖）。
    /// 没有 `SKILL.md` 的子目录会被跳过。加载失败收集到 diagnostics 中。
    pub fn load_dir(dir: impl AsRef<Path>) -> (Vec<SkillDef>, Vec<String>) {
        let mut defs = Vec::new();
        let mut diagnostics = Vec::new();
        let dir = dir.as_ref();

        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(e) => {
                diagnostics.push(format!("cannot read dir {}: {e}", dir.display()));
                return (defs, diagnostics);
            }
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let skill_file = path.join("SKILL.md");
            if !skill_file.exists() {
                continue;
            }
            // 目录名作为默认 skill name
            let dir_name = path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();

            match Skill::from_file_with_default_name(&skill_file, &dir_name) {
                Ok(d) => defs.push(d),
                Err(e) => diagnostics.push(e),
            }
        }

        (defs, diagnostics)
    }
}

fn skill_from_content(content: &str, default_name: &str, path: &Path) -> Result<SkillDef, String> {
    let (yaml, body) = parse_frontmatter(content);

    let fm: SkillFrontmatter = match yaml {
        Some(ref y) => serde_yaml::from_str(y)
            .map_err(|e| format!("invalid frontmatter in {}: {e}", path.display()))?,
        None => SkillFrontmatter { name: None, description: None, allowed_tools: vec![] },
    };

    let name = fm.name.unwrap_or_else(|| default_name.to_string());
    validate_name(&name).map_err(|e| format!("{}: {e}", path.display()))?;

    let description = fm.description
        .filter(|d| !d.trim().is_empty())
        .ok_or_else(|| format!("{}: description is required", path.display()))?;
    if description.len() > 1024 {
        return Err(format!("{}: description exceeds 1024 characters", path.display()));
    }

    let mut skill = Skill::new(name, description);
    if !body.is_empty() {
        skill.instructions.push(body);
    }

    Ok(SkillDef { skill, allowed_tools: fm.allowed_tools })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(filename: &str, content: &str) -> Result<SkillDef, String> {
        let path = std::env::temp_dir().join(filename);
        std::fs::write(&path, content).unwrap();
        Skill::from_file(&path)
    }

    #[test]
    fn test_full_frontmatter() {
        let d = parse("skill-full.md", "---\nname: my-skill\ndescription: Does something useful.\n---\n\nAlways be helpful.").unwrap();
        assert_eq!(d.skill.name, "my-skill");
        assert_eq!(d.skill.description, "Does something useful.");
        assert_eq!(d.skill.instructions, vec!["Always be helpful."]);
        assert!(d.allowed_tools.is_empty());
    }

    #[test]
    fn test_allowed_tools() {
        let d = parse("skill-tools.md", "---\nname: my-skill\ndescription: A skill.\nallowed-tools:\n  - calculator\n  - json_validate\n---\n\nUse tools.").unwrap();
        assert_eq!(d.allowed_tools, vec!["calculator", "json_validate"]);
    }

    #[test]
    fn test_default_name_from_filename() {
        let d = parse("test-skill.md", "---\ndescription: A skill.\n---\n\nBody.").unwrap();
        assert_eq!(d.skill.name, "test-skill");
    }

    #[test]
    fn test_missing_description_is_error() {
        assert!(parse("skill-nodesc.md", "---\nname: no-desc\n---\n\nBody.").is_err());
    }

    #[test]
    fn test_invalid_name() {
        assert!(parse("skill-badname.md", "---\nname: Bad_Name\ndescription: x\n---").is_err());
    }
}
