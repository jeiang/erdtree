#[cfg(unix)]
mod utils;

/// A root that is a link to a directory is walked, without `--follow`, as its target.
#[cfg(unix)]
#[test]
fn link() -> Result<(), Box<dyn std::error::Error>> {
    use indoc::indoc;
    use std::{os::unix::fs::symlink, path::Path};
    use tempfile::TempDir;

    let tmp = TempDir::new()?;
    let target = Path::new("./tests/data/the_yellow_king").canonicalize()?;
    let link = tmp.path().join("the_yellow_link");
    symlink(target, &link)?;

    assert_eq!(
        utils::run_cmd(&[link.to_str().unwrap()]),
        indoc!(
            "143 B ┌─ cassildas_song.md
            143 B the_yellow_king

            1 file"
        )
    );
    Ok(())
}
