use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

const SKILL_MD: &str = include_str!("../assets/skills/lineage/SKILL.md");

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Where the bundled skill is installed (per vendor docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SkillTarget {
    /// [Cursor](https://cursor.com/docs/skills) — `.cursor/skills/`
    Cursor,
    /// [Claude Code](https://code.claude.com/docs/en/claude-directory) — `.claude/skills/`
    Claude,
    /// [Codex](https://developers.openai.com/codex/concepts/customization) — `.agents/skills/`
    Codex,
}

impl SkillTarget {
    pub fn id(self) -> &'static str {
        match self {
            Self::Cursor => "cursor",
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    pub fn skill_path(self, repo_path: &Path) -> PathBuf {
        let subdir = match self {
            Self::Cursor => ".cursor/skills/lineage/SKILL.md",
            Self::Claude => ".claude/skills/lineage/SKILL.md",
            Self::Codex => ".agents/skills/lineage/SKILL.md",
        };
        repo_path.join(subdir)
    }

    pub fn doc_hint(self) -> &'static str {
        match self {
            Self::Cursor => "Cursor (.cursor/skills/)",
            Self::Claude => "Claude Code (.claude/skills/)",
            Self::Codex => "Codex (.agents/skills/)",
        }
    }
}

pub fn parse_skill_target(s: &str) -> std::result::Result<String, String> {
    match s.to_lowercase().as_str() {
        "cursor" | "claude" | "codex" | "agents" | "all" => Ok(s.to_lowercase()),
        other => Err(format!(
            "unknown skill target: {other} (use cursor, claude, codex, agents, or all)"
        )),
    }
}

pub fn resolve_targets(requested: &[String]) -> Vec<SkillTarget> {
    if requested.is_empty() || requested.iter().any(|t| t == "all") {
        return vec![SkillTarget::Cursor, SkillTarget::Claude, SkillTarget::Codex];
    }

    let mut set = HashSet::new();
    for t in requested {
        match t.as_str() {
            "cursor" => {
                set.insert(SkillTarget::Cursor);
            }
            "claude" => {
                set.insert(SkillTarget::Claude);
            }
            "codex" | "agents" => {
                set.insert(SkillTarget::Codex);
            }
            "all" => {
                return vec![SkillTarget::Cursor, SkillTarget::Claude, SkillTarget::Codex];
            }
            _ => {}
        }
    }

    let mut out: Vec<SkillTarget> = set.into_iter().collect();
    out.sort_by_key(|t| match t {
        SkillTarget::Cursor => 0,
        SkillTarget::Claude => 1,
        SkillTarget::Codex => 2,
    });
    out
}

pub fn init_skill(repo_path: &Path, targets: &[String], force: bool) -> Result<()> {
    let resolved = resolve_targets(targets);
    if resolved.is_empty() {
        return Err(
            "no skill targets selected (use --target cursor|claude|codex|agents|all)".into(),
        );
    }

    let mut existing = Vec::new();
    for target in &resolved {
        let path = target.skill_path(repo_path);
        if path.exists() && !force {
            existing.push(path);
        }
    }
    if !existing.is_empty() {
        let list = existing
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "skill already exists at: {list} (use --force to overwrite)"
        )
        .into());
    }

    println!("installing lineage agent skill:");
    for target in resolved {
        let path = target.skill_path(repo_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, SKILL_MD)?;
        println!("  {} — {}", path.display(), target.doc_hint());
    }
    println!("Agents can use lineage to search sessions, blame lines, and show conversations.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_all_by_default() {
        let t = resolve_targets(&[]);
        assert_eq!(t.len(), 3);
    }

    #[test]
    fn resolve_all_keyword() {
        let t = resolve_targets(&["all".into()]);
        assert_eq!(t.len(), 3);
    }

    #[test]
    fn agents_alias_maps_to_codex_path() {
        let t = resolve_targets(&["agents".into()]);
        assert_eq!(t, vec![SkillTarget::Codex]);
        assert!(t[0].skill_path(Path::new("/repo")).ends_with(".agents/skills/lineage/SKILL.md"));
    }

    #[test]
    fn resolve_multiselect() {
        let t = resolve_targets(&["cursor".into(), "claude".into()]);
        assert_eq!(t, vec![SkillTarget::Cursor, SkillTarget::Claude]);
    }
}
