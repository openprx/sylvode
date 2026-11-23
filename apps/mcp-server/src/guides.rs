//! The static guides the server embeds.
//!
//! The skill files are generated and checked by
//! `sylvode-skillgen` (`skills/sylvode-mcp`), so what the server serves is exactly what the
//! runner installs into a project.

/// `sylvode://skills/sylvode-mcp`: the skill with its generated tool overview.
pub const SKILL_MD: &str = include_str!("../../../skills/sylvode-mcp/SKILL.md");

/// `sylvode://skills/sylvode-mcp/tools`: every tool, one line each.
pub const TOOLS_MD: &str = include_str!("../../../skills/sylvode-mcp/reference/tools.md");

/// `sylvode://guides/workflows`: the recipes of the skill.
pub const RECIPES_MD: &str = include_str!("../../../skills/sylvode-mcp/generated/recipes.md");

/// The `instructions` of `initialize`.
pub const SERVER_INSTRUCTIONS: &str = include_str!("../../../skills/sylvode-mcp/generated/server-instructions.md");
