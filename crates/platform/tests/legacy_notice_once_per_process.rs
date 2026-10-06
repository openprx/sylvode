//! ADR-0020 D2: the legacy configuration notice is handed out once per process.
//!
//! Its own test binary, with a single test, because the notice state is process-global and the
//! test changes the working directory: no other test can observe or disturb either.

use platform::config::{OpenPrConfig, take_legacy_discovery_notice};

#[test]
fn the_legacy_discovery_notice_is_handed_out_exactly_once_per_process() -> Result<(), Box<dyn std::error::Error>> {
    let dir = std::env::temp_dir().join(format!("platform-legacy-notice-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("config"))?;
    std::fs::write(dir.join("config").join("openpr.toml"), "[logging]\nformat = \"text\"\n")?;
    std::env::set_current_dir(&dir)?;

    assert_eq!(take_legacy_discovery_notice(), None, "nothing was discovered yet");
    OpenPrConfig::load(None)?;
    let first = take_legacy_discovery_notice();
    let second = take_legacy_discovery_notice();
    OpenPrConfig::load(None)?;
    let after_reload = take_legacy_discovery_notice();

    let notice = first.ok_or("the first caller must get the notice")?;
    assert!(notice.contains("config/openpr.toml"), "{notice}");
    assert_eq!(second, None, "a second caller must not get the notice again");
    assert_eq!(after_reload, None, "loading again must not hand the notice out again");
    std::fs::remove_dir_all(&dir)?;
    Ok(())
}
