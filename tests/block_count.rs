//! The rendered `--disk-usage block` column. Block counts depend on the file system, so the
//! expected rows are built from what it reports for each file.
#![cfg(unix)]

use std::{fs, os::unix::fs::MetadataExt};
use tempfile::TempDir;

mod utils;

#[test]
fn block_count() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = TempDir::new()?;
    let root = tmp.path().join("r");
    fs::create_dir_all(root.join("sub"))?;
    fs::create_dir(root.join("empty"))?;
    fs::write(root.join("a"), "abc")?;
    fs::write(root.join("sub/big"), vec![7_u8; 100_000])?;
    fs::write(root.join("sub/e"), "")?;

    let blocks = |path: &str| fs::metadata(root.join(path)).map(|m| m.blocks());
    let (a, big, e) = (blocks("a")?, blocks("sub/big")?, blocks("sub/e")?);
    let (sub, total) = (big + e, a + big + e);

    let w = total.to_string().len();
    let expected = format!(
        "{e:>w$}    ┌─ e\n\
         {big:>w$}    ├─ big\n\
         {sub:>w$} ┌─ sub\n\
         {dash:>w$} ├─ empty\n\
         {a:>w$} ├─ a\n\
         {total:>w$} r\n\
         \n\
         2 directories, 3 files\n",
        dash = "-",
    );

    // Counts have no unit, so human-readable output is the same.
    for human in [&[][..], &["--human"]] {
        let mut args = vec!["--disk-usage", "block", root.to_str().unwrap()];
        args.extend_from_slice(human);
        assert_eq!(utils::run_cmd_untrimmed(&args), expected, "{human:?}");
    }
    Ok(())
}
