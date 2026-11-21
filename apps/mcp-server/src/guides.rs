//! The static guides the server embeds. The skill files are generated and checked by
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

/// `sylvode://guides/agents`: build and test notes for contributors; it names no tools.
pub const AGENTS_GUIDE_MD: &str = r#"# Sylvode Agent Guide

## Build
cargo build --release --bin mcp-server

## Test
curl -X POST http://localhost:8090/mcp/rpc -H "Content-Type: application/json" -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}"

## Commit Style: Conventional Commits (feat/fix/docs/ci)
"#;
