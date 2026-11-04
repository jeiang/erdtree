#[cfg(unix)]
mod utils;

/// `--follow` colors the branch of a row by state that rows below `--level` change too, so the
/// walk over the hidden entries must still happen.
#[cfg(unix)]
#[test]
fn follow_link_theme_sees_entries_below_level() -> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::fs::symlink;
    use tempfile::TempDir;

    let tmp = TempDir::new()?;
    let root = tmp.path().join("t");
    std::fs::create_dir_all(root.join("a/b"))?;
    std::fs::create_dir_all(root.join("c"))?;
    std::fs::create_dir_all(root.join("z"))?;
    std::fs::write(root.join("c/x"), "x")?;
    symlink("../../c", root.join("a/b/sd"))?;

    let root = root.to_string_lossy().into_owned();
    let out = utils::run_cmd_raw(&["--follow", "--level", "1", "-C", "force", &root], &[]);
    let out = String::from_utf8(out)?;

    // The hidden link a/b/sd -> c puts the row of `a` into the link theme.
    let row = out
        .lines()
        .find(|l| l.contains("\x1b[1;34ma"))
        .expect("row of a");
    assert!(row.contains("\x1b[31m"), "{row:?}");
    Ok(())
}
