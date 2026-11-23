use super::profile::{CLAUDE, CODEX, OPENCODE, profile};

#[test]
fn a_profile_is_found_by_its_name() {
    assert_eq!(profile("claude").map(|p| p.name), Some("claude"));
    assert_eq!(profile("codex").map(|p| p.name), Some("codex"));
    assert_eq!(profile("opencode").map(|p| p.name), Some("opencode"));
    assert!(profile("other").is_none() && profile("").is_none() && profile("Claude").is_none());
}

#[test]
fn the_glyphs_and_markers_are_the_verified_ones() {
    assert_eq!(CLAUDE.prompt_glyphs, ['>', '\u{276f}']);
    assert_eq!(CODEX.prompt_glyphs, ['\u{203a}']);
    assert_eq!(OPENCODE.prompt_glyphs, ['>']);
    assert!(CLAUDE.has_pasted_marker("[Sylvode task] id=1"));
    assert!(!CLAUDE.has_pasted_marker("[Pasted Content 1663 chars]"));
    assert!(CODEX.has_pasted_marker("[Sylvode task] id=1"));
    assert!(CODEX.has_pasted_marker("[Pasted Content 1663 chars]"));
    assert!(!CODEX.has_pasted_marker("run tests") && !CODEX.has_pasted_marker(""));
}
