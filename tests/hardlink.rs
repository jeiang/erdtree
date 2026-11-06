use indoc::indoc;
use std::{error::Error, fs};

mod utils;

#[test]
fn hardlink() -> Result<(), Box<dyn Error>> {
    let tmp = tempfile::TempDir::new()?;
    let root = tmp.path().join("hardlinks");
    fs::create_dir(&root)?;
    fs::write(
        root.join("kadath.txt"),
        "“It was a song, but not the song of any voice. Night and the spheres sang it, and it was old when space and Nyarlathotep and the Other Gods were born.” \n",
    )?;
    fs::hard_link(root.join("kadath.txt"), root.join("curwin.hpl"))?;

    let out = utils::run_cmd(&[&root.display().to_string()]);

    assert_eq!(
        out,
        indoc!(
            "157 B ┌─ kadath.txt
 157 B ├─ curwin.hpl
 157 B hardlinks

 2 files"
        )
    );

    Ok(())
}

/// Rows of `erd -y iflat --no-report <args> <dir>` below the root.
fn rows_below_root(dir: &std::path::Path, args: &[&str]) -> String {
    let dir = dir.display().to_string();
    let mut all = vec!["-y", "iflat", "--no-report"];
    all.extend_from_slice(args);
    all.push(&dir);

    let out = utils::run_cmd(&all);
    out.lines().skip(1).collect::<Vec<_>>().join("\n")
}

/// An inode with several links counts towards the directory of the link with the least path,
/// whatever order the directories are read in.
#[test]
fn hard_link_counts_in_least_path() -> Result<(), Box<dyn Error>> {
    let tmp = tempfile::TempDir::new()?;
    fs::create_dir_all(tmp.path().join("x"))?;
    fs::create_dir_all(tmp.path().join("y"))?;
    fs::write(tmp.path().join("x/f"), "0123456789")?;
    fs::hard_link(tmp.path().join("x/f"), tmp.path().join("y/g"))?;

    assert_eq!(
        rows_below_root(tmp.path(), &[]),
        ["10 B   x", "10 B   x/f", "   -   y", "10 B   y/g"].join("\n")
    );

    Ok(())
}

/// With `--follow`, a directory reached again through a symlink counts towards the directory
/// that holds it, not towards the directory of the link.
#[cfg(unix)]
#[test]
fn followed_directory_counts_in_real_location() -> Result<(), Box<dyn Error>> {
    let tmp = tempfile::TempDir::new()?;
    fs::create_dir_all(tmp.path().join("a/real"))?;
    fs::create_dir_all(tmp.path().join("b"))?;
    fs::write(tmp.path().join("a/real/f"), "hello world\n")?;
    std::os::unix::fs::symlink("../a/real", tmp.path().join("b/link"))?;

    assert_eq!(
        rows_below_root(tmp.path(), &["--follow"]),
        [
            "12 B   a",
            "12 B   a/real",
            "12 B   a/real/f",
            "   -   b",
            "12 B   b/link",
            "12 B   b/link/f",
        ]
        .join("\n")
    );

    Ok(())
}

/// Entries of equal size are ordered by name.
#[test]
fn size_ties_ordered_by_name() -> Result<(), Box<dyn Error>> {
    let tmp = tempfile::TempDir::new()?;
    let x = tmp.path().join("x");
    fs::create_dir_all(&x)?;
    for name in ["c", "a", "b"] {
        fs::write(x.join(name), "0123456789")?;
    }

    let x = x.display().to_string();
    let out = utils::run_cmd(&["-y", "iflat", "--no-report", "--sort", "size", &x]);

    assert_eq!(out, "30 B   x\n10 B   a\n10 B   b\n10 B   c");

    Ok(())
}
