#[cfg(unix)]
mod utils;

/// Stdout of `erd -C force args`, escapes included.
#[cfg(unix)]
fn colored(args: &[&str]) -> String {
    utils::Erd::new()
        .args(["-C", "force"])
        .args(args)
        .ok()
        .raw()
}

/// `out` without escapes, each row in the link theme marked with a trailing ` *`.
#[cfg(unix)]
fn mark_link_theme(out: &str) -> String {
    out.trim_end()
        .lines()
        .map(|line| {
            let text = String::from_utf8(strip_ansi_escapes::strip(line)).unwrap();
            let mark = if utils::in_link_theme(line) { " *" } else { "" };
            format!("{text}{mark}")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `--follow` paints only the rows below a followed link in the link theme, so a link hidden by
/// `--level` leaves the row of its shown ancestor in the tree theme.
#[cfg(unix)]
#[test]
fn follow_link_theme_skips_ancestors_of_hidden_link() -> Result<(), Box<dyn std::error::Error>> {
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
    let out = colored(&["--follow", "--level", "1", &root]);

    // The hidden link a/b/sd -> c is below `a`, not above it.
    let row = out
        .lines()
        .find(|l| l.contains("\x1b[1;34ma"))
        .expect("row of a");
    assert!(!row.contains("\x1b[31m"), "{row:?}");
    Ok(())
}

/// Rows of `--follow -C force` over a tree with a link to a directory, after `filter` hides
/// children in the middle of their directories, marked by [`mark_link_theme`].
#[cfg(unix)]
fn follow_rows(filter: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
    use std::os::unix::fs::symlink;
    use tempfile::TempDir;

    let tmp = TempDir::new()?;
    let root = tmp.path().join("t");
    for dir in ["a/b", "c", "e", "m", "z"] {
        std::fs::create_dir_all(root.join(dir))?;
    }
    std::fs::write(root.join("c/big"), [0; 200])?;
    std::fs::write(root.join("c/x"), "x\n")?;
    std::fs::write(root.join("c/zz"), [0; 50])?;
    std::fs::write(root.join("z/y"), "yy\n")?;
    symlink("../../c", root.join("a/b/sd"))?;

    let root = root.to_string_lossy().into_owned();
    let mut args = vec!["--follow"];
    args.extend_from_slice(filter);
    args.push(&root);

    Ok(mark_link_theme(&colored(&args)))
}

/// `--prune` hides the empty directories between `c` and `z`; the link theme covers exactly the
/// rows of the link and below it, not its ancestors.
#[cfg(unix)]
#[test]
fn follow_with_prune_hiding_middle_children() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        follow_rows(&["--prune"])?,
        indoc::indoc!(
            "
              3 B    ┌─ y
              3 B ┌─ z
             50 B │  ┌─ zz
              2 B │  ├─ x
            200 B │  ├─ big
            252 B ├─ c
             50 B │        ┌─ zz *
              2 B │        ├─ x *
            200 B │        ├─ big *
            252 B │     ┌─ sd → ../../c *
                - │  ┌─ b
                - ├─ a
            255 B t

            5 directories, 7 files"
        )
        .trim_matches('\n')
    );
    Ok(())
}

/// `--min-size` hides `x` between `big` and `zz`, in `c` and below the link.
#[cfg(unix)]
#[test]
fn follow_with_min_size_hiding_middle_children() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        follow_rows(&["--min-size", "3"])?,
        indoc::indoc!(
            "
              3 B    ┌─ y
              3 B ┌─ z
             50 B │  ┌─ zz
            200 B │  ├─ big
            252 B ├─ c
             50 B │        ┌─ zz *
            200 B │        ├─ big *
            252 B │     ┌─ sd → ../../c *
                - │  ┌─ b
                - ├─ a
            255 B t

            7 directories, 7 files"
        )
        .trim_matches('\n')
    );
    Ok(())
}

/// The regular layout draws `0`, which precedes `a` in pre-order, below the link; leaving the
/// link's subtree through the shallower `b` and `a` ends the link theme.
#[cfg(unix)]
#[test]
fn follow_link_theme_ends_after_subtree() -> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::fs::symlink;
    use tempfile::TempDir;

    let tmp = TempDir::new()?;
    let root = tmp.path().join("t");
    std::fs::create_dir_all(root.join("0"))?;
    std::fs::create_dir_all(root.join("a/b"))?;
    std::fs::create_dir_all(root.join("c"))?;
    std::fs::write(root.join("0/x"), "x")?;
    std::fs::write(root.join("c/x"), "x")?;
    symlink("../../c", root.join("a/b/sd"))?;

    let root = root.to_string_lossy().into_owned();
    let rows = mark_link_theme(&colored(&["--follow", &root]));

    assert_eq!(
        rows,
        indoc::indoc!(
            "
            1 B    ┌─ x
            1 B ┌─ c
            1 B │        ┌─ x *
            1 B │     ┌─ sd → ../../c *
              - │  ┌─ b
              - ├─ a
            1 B │  ┌─ x
            1 B ├─ 0
            2 B t

            5 directories, 3 files"
        )
        .trim_matches('\n')
    );
    Ok(())
}

/// The inverted layout paints the branches of a followed link and the rows below it in the link
/// theme, byte for byte; `c` and `z`, after the link but not below it, keep the tree theme.
#[cfg(unix)]
#[test]
fn follow_link_theme_inverted_golden() -> Result<(), Box<dyn std::error::Error>> {
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
    let out = colored(&["--follow", "-y", "inverted", &root]);

    let expected = concat!(
        "\x1b[1;36m1 B\x1b[0m \x1b[1;34mt\x1b[0m\n",
        "\x1b[35m  -\x1b[0m \x1b[35m├─ \x1b[0m\x1b[1;34ma\x1b[0m\n",
        "\x1b[35m  -\x1b[0m \x1b[35m│  \x1b[0m\x1b[35m└─ \x1b[0m\x1b[1;34mb\x1b[0m\n",
        "\x1b[1;36m1 B\x1b[0m \x1b[35m│  \x1b[0m   \x1b[31m└─ \x1b[0m\x1b[1;34msd\x1b[0m \x1b[31m→ ../../c\x1b[0m\n",
        "\x1b[1;36m1 B\x1b[0m \x1b[35m│  \x1b[0m      \x1b[31m└─ \x1b[0mx\n",
        "\x1b[1;36m1 B\x1b[0m \x1b[35m├─ \x1b[0m\x1b[1;34mc\x1b[0m\n",
        "\x1b[1;36m1 B\x1b[0m \x1b[35m│  \x1b[0m\x1b[35m└─ \x1b[0mx\n",
        "\x1b[35m  -\x1b[0m \x1b[35m└─ \x1b[0m\x1b[1;34mz\x1b[0m\n",
        "\n",
        "5 directories, 2 files\n",
    );
    assert_eq!(out, expected);
    Ok(())
}
