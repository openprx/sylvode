//! `list-tools --json` prints name, access and category of every registry tool. The golden file
//! is reviewed with every registry or classification change.

use std::process::Command;

fn list_tools(arguments: &[&str]) -> Result<std::process::Output, std::io::Error> {
    Command::new(env!("CARGO_BIN_EXE_list-tools")).args(arguments).output()
}

#[test]
fn json_listing_matches_the_golden_file() -> Result<(), Box<dyn std::error::Error>> {
    let output = list_tools(&["--json"])?;
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let golden = include_str!("golden/list-tools.json");
    assert_eq!(String::from_utf8(output.stdout)?, golden);

    let rows: serde_json::Value = serde_json::from_str(golden)?;
    let rows = rows.as_array().ok_or("the listing is not an array")?;
    assert_eq!(rows.len(), mcp_server::get_all_tool_definitions().len());
    for row in rows {
        let access = row["access"].as_str().unwrap_or_default();
        assert!(matches!(access, "read" | "write"), "{row}");
        assert!(
            row["category"].as_str().is_some_and(|category| !category.is_empty()),
            "{row}"
        );
    }
    Ok(())
}

#[test]
fn an_unknown_argument_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    let output = list_tools(&["--yaml"])?;
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    Ok(())
}

#[test]
fn the_plain_listing_still_counts_the_registry() -> Result<(), Box<dyn std::error::Error>> {
    let output = list_tools(&[])?;
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout)?;
    let count = mcp_server::get_all_tool_definitions().len();
    assert!(
        text.starts_with(&format!("Available MCP Tools ({count} total):")),
        "{}",
        &text[..60]
    );
    Ok(())
}
