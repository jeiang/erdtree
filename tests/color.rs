//! Byte-exact goldens of forced-colour output: the long view, the size column with its unit
//! colors and placeholder, and block counts.

#[cfg(unix)]
mod utils;

#[cfg(unix)]
mod colored {
    use super::utils;
    use std::{
        fs::{self, File},
        os::unix::fs::{MetadataExt, PermissionsExt},
        path::Path,
        sync::OnceLock,
        time::{Duration, UNIX_EPOCH},
    };
    use tempfile::TempDir;

    /// 2024-01-15 12:00:00 UTC.
    const STAMP: u64 = 1_705_320_000;

    /// A tree with every unit color among its sizes (sparse files have logical sizes without
    /// blocks), a file `f` and a sticky, world-writable directory. Every entry and the root
    /// share one modification time.
    pub(super) fn fixture() -> Result<(TempDir, String), Box<dyn std::error::Error>> {
        fixture_with_f_mode(0o644)
    }

    /// [`fixture`] with `f` in `f_mode`.
    pub(super) fn fixture_with_f_mode(
        f_mode: u32,
    ) -> Result<(TempDir, String), Box<dyn std::error::Error>> {
        let tmp = TempDir::new()?;
        let root = tmp.path().join("t");
        let mode = |path: &Path, mode| fs::set_permissions(path, fs::Permissions::from_mode(mode));

        fs::create_dir_all(root.join("d"))?;
        mode(&root, 0o755)?;
        mode(&root.join("d"), 0o755)?;
        fs::create_dir(root.join("s"))?;
        mode(&root.join("s"), 0o1777)?;
        fs::write(root.join("e"), "")?;
        fs::write(root.join("f"), "abc")?;
        mode(&root.join("f"), f_mode)?;
        fs::write(root.join("m"), vec![0u8; 2 << 20])?;
        File::create(root.join("k"))?.set_len(1_500)?;
        File::create(root.join("g"))?.set_len(3 << 30)?;
        File::create(root.join("h"))?.set_len(1_100_000_000_000)?;
        for name in ["e", "m", "k", "g", "h"] {
            mode(&root.join(name), 0o644)?;
        }

        for name in ["d", "s", "e", "f", "m", "k", "g", "h", ""] {
            File::open(root.join(name))?.set_modified(UNIX_EPOCH + Duration::from_secs(STAMP))?;
        }

        let root = root.to_string_lossy().into_owned();
        Ok((tmp, root))
    }

    pub(super) fn run(args: &[&str]) -> String {
        utils::Erd::new().args(args).env("TZ", "UTC").ok().raw()
    }

    /// The owner and group names of `path`, which vary between machines.
    fn names(path: &str) -> &'static (String, String) {
        static NAMES: OnceLock<(String, String)> = OnceLock::new();

        NAMES.get_or_init(|| {
            let meta = fs::metadata(path).unwrap();
            // SAFETY: `getpwuid`/`getgrgid` return null or a record that stays valid until the
            // next call; `OnceLock` makes this the only caller, and the name is copied at once.
            unsafe {
                let pw = libc::getpwuid(meta.uid());
                let gr = libc::getgrgid(meta.gid());
                assert!(!pw.is_null() && !gr.is_null());
                (
                    std::ffi::CStr::from_ptr((*pw).pw_name)
                        .to_string_lossy()
                        .into_owned(),
                    std::ffi::CStr::from_ptr((*gr).gr_name)
                        .to_string_lossy()
                        .into_owned(),
                )
            }
        })
    }

    /// Replaces what varies between machines: owner and group names, and the extended-attribute
    /// cell, which is `@` or a space depending on the file system; both styles are matched exactly.
    pub(super) fn normalize(out: &str, root: &str) -> String {
        let (owner, group) = names(root);
        // By style too: owner and group often share a name (runner/runner on CI).
        out.replace(
            &format!("\x1b[1;36m{owner}\x1b[0m"),
            "\x1b[1;36mOWNER\x1b[0m",
        )
        .replace(
            &format!("\x1b[1;32m{group}\x1b[0m"),
            "\x1b[1;32mGROUP\x1b[0m",
        )
        .replace("\x1b[1;36m@\x1b[0m", "<x>")
        .replace("\x1b[37m \x1b[0m", "<x>")
    }

    /// Replaces the digits and padding of every cell opened by `pre` that holds only those with
    /// `#`: inode numbers and link counts, which vary between machines and file systems.
    pub(super) fn mask(out: &str, pre: &str) -> String {
        let mut masked = String::with_capacity(out.len());
        let mut rest = out;

        while let Some(at) = rest.find(pre) {
            let (head, tail) = rest.split_at(at + pre.len());
            masked.push_str(head);
            rest = tail;

            if let Some(end) = rest.find("\x1b[0m") {
                let cell = &rest[..end];
                if !cell.is_empty() && cell.bytes().all(|b| b.is_ascii_digit() || b == b' ') {
                    masked.push('#');
                    rest = &rest[end..];
                }
            }
        }

        masked.push_str(rest);
        masked
    }

    /// Symbolic permissions with a style per character, owner and time styles.
    #[test]
    fn long_symbolic() -> Result<(), Box<dyn std::error::Error>> {
        let (_tmp, root) = fixture()?;
        let out = normalize(&run(&["--long", "-C", "force", &root]), &root);

        let expected = concat!(
            "\x1b[1;34md\x1b[0m\x1b[1;32mr\x1b[0m\x1b[1;33mw\x1b[0m\x1b[1;31mx\x1b[0m\x1b[1;32mr\x1b[0m\x1b[1;33mw\x1b[0m\x1b[1;31mx\x1b[0m\x1b[1;32mr\x1b[0m\x1b[1;33mw\x1b[0m\x1b[1;31mt\x1b[0m<x> \x1b[1;36mOWNER\x1b[0m \x1b[1;35m15 Jan 12:00 24\x1b[0m \x1b[35m              -\x1b[0m \x1b[35m┌─ \x1b[0m\x1b[1;42;30ms\x1b[0m\n",
            "\x1b[35m.\x1b[0m\x1b[1;32mr\x1b[0m\x1b[1;33mw\x1b[0m\x1b[35m-\x1b[0m\x1b[1;32mr\x1b[0m\x1b[35m-\x1b[0m\x1b[35m-\x1b[0m\x1b[1;32mr\x1b[0m\x1b[35m-\x1b[0m\x1b[35m-\x1b[0m<x> \x1b[1;36mOWNER\x1b[0m \x1b[1;35m15 Jan 12:00 24\x1b[0m \x1b[1;32m      2097152 B\x1b[0m \x1b[35m├─ \x1b[0mm\n",
            "\x1b[35m.\x1b[0m\x1b[1;32mr\x1b[0m\x1b[1;33mw\x1b[0m\x1b[35m-\x1b[0m\x1b[1;32mr\x1b[0m\x1b[35m-\x1b[0m\x1b[35m-\x1b[0m\x1b[1;32mr\x1b[0m\x1b[35m-\x1b[0m\x1b[35m-\x1b[0m<x> \x1b[1;36mOWNER\x1b[0m \x1b[1;35m15 Jan 12:00 24\x1b[0m \x1b[1;33m         1500 B\x1b[0m \x1b[35m├─ \x1b[0mk\n",
            "\x1b[35m.\x1b[0m\x1b[1;32mr\x1b[0m\x1b[1;33mw\x1b[0m\x1b[35m-\x1b[0m\x1b[1;32mr\x1b[0m\x1b[35m-\x1b[0m\x1b[35m-\x1b[0m\x1b[1;32mr\x1b[0m\x1b[35m-\x1b[0m\x1b[35m-\x1b[0m<x> \x1b[1;36mOWNER\x1b[0m \x1b[1;35m15 Jan 12:00 24\x1b[0m \x1b[1;34m1100000000000 B\x1b[0m \x1b[35m├─ \x1b[0mh\n",
            "\x1b[35m.\x1b[0m\x1b[1;32mr\x1b[0m\x1b[1;33mw\x1b[0m\x1b[35m-\x1b[0m\x1b[1;32mr\x1b[0m\x1b[35m-\x1b[0m\x1b[35m-\x1b[0m\x1b[1;32mr\x1b[0m\x1b[35m-\x1b[0m\x1b[35m-\x1b[0m<x> \x1b[1;36mOWNER\x1b[0m \x1b[1;35m15 Jan 12:00 24\x1b[0m \x1b[1;31m   3221225472 B\x1b[0m \x1b[35m├─ \x1b[0mg\n",
            "\x1b[35m.\x1b[0m\x1b[1;32mr\x1b[0m\x1b[1;33mw\x1b[0m\x1b[35m-\x1b[0m\x1b[1;32mr\x1b[0m\x1b[35m-\x1b[0m\x1b[35m-\x1b[0m\x1b[1;32mr\x1b[0m\x1b[35m-\x1b[0m\x1b[35m-\x1b[0m<x> \x1b[1;36mOWNER\x1b[0m \x1b[1;35m15 Jan 12:00 24\x1b[0m \x1b[1;36m            3 B\x1b[0m \x1b[35m├─ \x1b[0mf\n",
            "\x1b[35m.\x1b[0m\x1b[1;32mr\x1b[0m\x1b[1;33mw\x1b[0m\x1b[35m-\x1b[0m\x1b[1;32mr\x1b[0m\x1b[35m-\x1b[0m\x1b[35m-\x1b[0m\x1b[1;32mr\x1b[0m\x1b[35m-\x1b[0m\x1b[35m-\x1b[0m<x> \x1b[1;36mOWNER\x1b[0m \x1b[1;35m15 Jan 12:00 24\x1b[0m \x1b[1;36m            0 B\x1b[0m \x1b[35m├─ \x1b[0me\n",
            "\x1b[1;34md\x1b[0m\x1b[1;32mr\x1b[0m\x1b[1;33mw\x1b[0m\x1b[1;31mx\x1b[0m\x1b[1;32mr\x1b[0m\x1b[35m-\x1b[0m\x1b[1;31mx\x1b[0m\x1b[1;32mr\x1b[0m\x1b[35m-\x1b[0m\x1b[1;31mx\x1b[0m<x> \x1b[1;36mOWNER\x1b[0m \x1b[1;35m15 Jan 12:00 24\x1b[0m \x1b[35m              -\x1b[0m \x1b[35m├─ \x1b[0m\x1b[1;34md\x1b[0m\n",
            "\x1b[1;34md\x1b[0m\x1b[1;32mr\x1b[0m\x1b[1;33mw\x1b[0m\x1b[1;31mx\x1b[0m\x1b[1;32mr\x1b[0m\x1b[35m-\x1b[0m\x1b[1;31mx\x1b[0m\x1b[1;32mr\x1b[0m\x1b[35m-\x1b[0m\x1b[1;31mx\x1b[0m<x> \x1b[1;36mOWNER\x1b[0m \x1b[1;35m15 Jan 12:00 24\x1b[0m \x1b[1;34m1103223324127 B\x1b[0m \x1b[1;34mt\x1b[0m\n",
            "\n",
            "2 directories, 6 files\n",
        );
        assert_eq!(out, expected, "{out}");
        Ok(())
    }

    /// Octal permissions, inode, link count, owner and group styles.
    #[test]
    fn long_octal() -> Result<(), Box<dyn std::error::Error>> {
        let (_tmp, root) = fixture()?;
        let out = mask(
            &mask(
                &normalize(
                    &run(&[
                        "--long", "--octal", "--ino", "--nlink", "--group", "-C", "force", &root,
                    ]),
                    &root,
                ),
                "\x1b[1;36m",
            ),
            "\x1b[1;31m",
        );

        let expected = concat!(
            "\x1b[1;36m#\x1b[0m \x1b[1;35m1777\x1b[0m \x1b[1;31m#\x1b[0m \x1b[1;36mOWNER\x1b[0m \x1b[1;32mGROUP\x1b[0m \x1b[1;35m15 Jan 12:00 24\x1b[0m \x1b[35m              -\x1b[0m \x1b[35m┌─ \x1b[0m\x1b[1;42;30ms\x1b[0m\n",
            "\x1b[1;36m#\x1b[0m \x1b[1;35m0644\x1b[0m \x1b[1;31m#\x1b[0m \x1b[1;36mOWNER\x1b[0m \x1b[1;32mGROUP\x1b[0m \x1b[1;35m15 Jan 12:00 24\x1b[0m \x1b[1;32m      2097152 B\x1b[0m \x1b[35m├─ \x1b[0mm\n",
            "\x1b[1;36m#\x1b[0m \x1b[1;35m0644\x1b[0m \x1b[1;31m#\x1b[0m \x1b[1;36mOWNER\x1b[0m \x1b[1;32mGROUP\x1b[0m \x1b[1;35m15 Jan 12:00 24\x1b[0m \x1b[1;33m         1500 B\x1b[0m \x1b[35m├─ \x1b[0mk\n",
            "\x1b[1;36m#\x1b[0m \x1b[1;35m0644\x1b[0m \x1b[1;31m#\x1b[0m \x1b[1;36mOWNER\x1b[0m \x1b[1;32mGROUP\x1b[0m \x1b[1;35m15 Jan 12:00 24\x1b[0m \x1b[1;34m1100000000000 B\x1b[0m \x1b[35m├─ \x1b[0mh\n",
            "\x1b[1;36m#\x1b[0m \x1b[1;35m0644\x1b[0m \x1b[1;31m#\x1b[0m \x1b[1;36mOWNER\x1b[0m \x1b[1;32mGROUP\x1b[0m \x1b[1;35m15 Jan 12:00 24\x1b[0m \x1b[1;31m   3221225472 B\x1b[0m \x1b[35m├─ \x1b[0mg\n",
            "\x1b[1;36m#\x1b[0m \x1b[1;35m0644\x1b[0m \x1b[1;31m#\x1b[0m \x1b[1;36mOWNER\x1b[0m \x1b[1;32mGROUP\x1b[0m \x1b[1;35m15 Jan 12:00 24\x1b[0m \x1b[1;36m            3 B\x1b[0m \x1b[35m├─ \x1b[0mf\n",
            "\x1b[1;36m#\x1b[0m \x1b[1;35m0644\x1b[0m \x1b[1;31m#\x1b[0m \x1b[1;36mOWNER\x1b[0m \x1b[1;32mGROUP\x1b[0m \x1b[1;35m15 Jan 12:00 24\x1b[0m \x1b[1;36m            0 B\x1b[0m \x1b[35m├─ \x1b[0me\n",
            "\x1b[1;36m#\x1b[0m \x1b[1;35m0755\x1b[0m \x1b[1;31m#\x1b[0m \x1b[1;36mOWNER\x1b[0m \x1b[1;32mGROUP\x1b[0m \x1b[1;35m15 Jan 12:00 24\x1b[0m \x1b[35m              -\x1b[0m \x1b[35m├─ \x1b[0m\x1b[1;34md\x1b[0m\n",
            "\x1b[1;36m#\x1b[0m \x1b[1;35m0755\x1b[0m \x1b[1;31m#\x1b[0m \x1b[1;36mOWNER\x1b[0m \x1b[1;32mGROUP\x1b[0m \x1b[1;35m15 Jan 12:00 24\x1b[0m \x1b[1;34m1103223324127 B\x1b[0m \x1b[1;34mt\x1b[0m\n",
            "\n",
            "2 directories, 6 files\n",
        );
        assert_eq!(out, expected, "{out}");
        Ok(())
    }

    /// Byte counts colored by the unit their magnitude falls in, and the placeholder of
    /// directories without a size.
    #[test]
    fn du_bytes() -> Result<(), Box<dyn std::error::Error>> {
        let (_tmp, root) = fixture()?;
        let out = run(&["-C", "force", &root]);

        let expected = concat!(
            "\x1b[35m              -\x1b[0m \x1b[35m┌─ \x1b[0m\x1b[1;42;30ms\x1b[0m\n",
            "\x1b[1;32m      2097152 B\x1b[0m \x1b[35m├─ \x1b[0mm\n",
            "\x1b[1;33m         1500 B\x1b[0m \x1b[35m├─ \x1b[0mk\n",
            "\x1b[1;34m1100000000000 B\x1b[0m \x1b[35m├─ \x1b[0mh\n",
            "\x1b[1;31m   3221225472 B\x1b[0m \x1b[35m├─ \x1b[0mg\n",
            "\x1b[1;36m            3 B\x1b[0m \x1b[35m├─ \x1b[0mf\n",
            "\x1b[1;36m            0 B\x1b[0m \x1b[35m├─ \x1b[0me\n",
            "\x1b[35m              -\x1b[0m \x1b[35m├─ \x1b[0m\x1b[1;34md\x1b[0m\n",
            "\x1b[1;34m1103223324127 B\x1b[0m \x1b[1;34mt\x1b[0m\n",
            "\n",
            "2 directories, 6 files\n",
        );
        assert_eq!(out, expected, "{out}");
        Ok(())
    }

    /// SI units, each in its color.
    #[test]
    fn du_si() -> Result<(), Box<dyn std::error::Error>> {
        let (_tmp, root) = fixture()?;
        let out = run(&["-C", "force", "-H", "--unit", "si", &root]);

        let expected = concat!(
            "\x1b[35m     -\x1b[0m \x1b[35m┌─ \x1b[0m\x1b[1;42;30ms\x1b[0m\n",
            "\x1b[1;32m2.1 MB\x1b[0m \x1b[35m├─ \x1b[0mm\n",
            "\x1b[1;33m1.5 KB\x1b[0m \x1b[35m├─ \x1b[0mk\n",
            "\x1b[1;34m1.1 TB\x1b[0m \x1b[35m├─ \x1b[0mh\n",
            "\x1b[1;31m3.2 GB\x1b[0m \x1b[35m├─ \x1b[0mg\n",
            "\x1b[1;36m  3  B\x1b[0m \x1b[35m├─ \x1b[0mf\n",
            "\x1b[1;36m  0  B\x1b[0m \x1b[35m├─ \x1b[0me\n",
            "\x1b[35m     -\x1b[0m \x1b[35m├─ \x1b[0m\x1b[1;34md\x1b[0m\n",
            "\x1b[1;34m1.1 TB\x1b[0m \x1b[1;34mt\x1b[0m\n",
            "\n",
            "2 directories, 6 files\n",
        );
        assert_eq!(out, expected, "{out}");
        Ok(())
    }

    /// Binary units, each in its color.
    #[test]
    fn du_bin() -> Result<(), Box<dyn std::error::Error>> {
        let (_tmp, root) = fixture()?;
        let out = run(&["-C", "force", "-H", "--unit", "bin", &root]);

        let expected = concat!(
            "\x1b[35m      -\x1b[0m \x1b[35m┌─ \x1b[0m\x1b[1;42;30ms\x1b[0m\n",
            "\x1b[1;32m2.0 MiB\x1b[0m \x1b[35m├─ \x1b[0mm\n",
            "\x1b[1;33m1.5 KiB\x1b[0m \x1b[35m├─ \x1b[0mk\n",
            "\x1b[1;34m1.0 TiB\x1b[0m \x1b[35m├─ \x1b[0mh\n",
            "\x1b[1;31m3.0 GiB\x1b[0m \x1b[35m├─ \x1b[0mg\n",
            "\x1b[1;36m  3   B\x1b[0m \x1b[35m├─ \x1b[0mf\n",
            "\x1b[1;36m  0   B\x1b[0m \x1b[35m├─ \x1b[0me\n",
            "\x1b[35m      -\x1b[0m \x1b[35m├─ \x1b[0m\x1b[1;34md\x1b[0m\n",
            "\x1b[1;34m1.0 TiB\x1b[0m \x1b[1;34mt\x1b[0m\n",
            "\n",
            "2 directories, 6 files\n",
        );
        assert_eq!(out, expected, "{out}");
        Ok(())
    }

    /// Block counts, colored by the unit of `blocks * 512` bytes.
    #[test]
    fn block() -> Result<(), Box<dyn std::error::Error>> {
        let (_tmp, root) = fixture()?;
        let out = run(&["-d", "block", "-C", "force", &root]);

        let expected = concat!(
            "\x1b[35m   -\x1b[0m \x1b[35m┌─ \x1b[0m\x1b[1;42;30ms\x1b[0m\n",
            "\x1b[1;32m4096\x1b[0m \x1b[35m├─ \x1b[0mm\n",
            "\x1b[1;36m   0\x1b[0m \x1b[35m├─ \x1b[0mk\n",
            "\x1b[1;36m   0\x1b[0m \x1b[35m├─ \x1b[0mh\n",
            "\x1b[1;36m   0\x1b[0m \x1b[35m├─ \x1b[0mg\n",
            "\x1b[1;33m   8\x1b[0m \x1b[35m├─ \x1b[0mf\n",
            "\x1b[1;36m   0\x1b[0m \x1b[35m├─ \x1b[0me\n",
            "\x1b[35m   -\x1b[0m \x1b[35m├─ \x1b[0m\x1b[1;34md\x1b[0m\n",
            "\x1b[1;32m4104\x1b[0m \x1b[1;34mt\x1b[0m\n",
            "\n",
            "2 directories, 6 files\n",
        );
        assert_eq!(out, expected, "{out}");
        Ok(())
    }
}

/// The setuid bit, which the Nix sandbox forbids setting; `flake.nix` skips `setuid_bit::`.
#[cfg(unix)]
mod setuid_bit {
    use super::colored::{fixture_with_f_mode, mask, normalize, run};

    /// The row of `f` in `out`.
    fn f_row(out: &str) -> &str {
        out.lines()
            .find(|l| l.ends_with("f\x1b[0m"))
            .unwrap_or_else(|| panic!("no row f: {out}"))
    }

    #[test]
    fn long_symbolic() -> Result<(), Box<dyn std::error::Error>> {
        let (_tmp, root) = fixture_with_f_mode(0o4755)?;
        let out = normalize(&run(&["--long", "-C", "force", &root]), &root);

        assert_eq!(
            f_row(&out),
            "\x1b[35m.\x1b[0m\x1b[1;32mr\x1b[0m\x1b[1;33mw\x1b[0m\x1b[1;31ms\x1b[0m\x1b[1;32mr\x1b[0m\x1b[35m-\x1b[0m\x1b[1;31mx\x1b[0m\x1b[1;32mr\x1b[0m\x1b[35m-\x1b[0m\x1b[1;31mx\x1b[0m<x> \x1b[1;36mOWNER\x1b[0m \x1b[1;35m15 Jan 12:00 24\x1b[0m \x1b[1;36m            3 B\x1b[0m \x1b[35m├─ \x1b[0m\x1b[1;41;37mf\x1b[0m"
        );
        Ok(())
    }

    #[test]
    fn long_octal() -> Result<(), Box<dyn std::error::Error>> {
        let (_tmp, root) = fixture_with_f_mode(0o4755)?;
        let out = run(&[
            "--long", "--octal", "--ino", "--nlink", "--group", "-C", "force", &root,
        ]);
        let out = mask(&mask(&normalize(&out, &root), "\x1b[1;36m"), "\x1b[1;31m");

        assert_eq!(
            f_row(&out),
            "\x1b[1;36m#\x1b[0m \x1b[1;35m4755\x1b[0m \x1b[1;31m#\x1b[0m \x1b[1;36mOWNER\x1b[0m \x1b[1;32mGROUP\x1b[0m \x1b[1;35m15 Jan 12:00 24\x1b[0m \x1b[1;36m            3 B\x1b[0m \x1b[35m├─ \x1b[0m\x1b[1;41;37mf\x1b[0m"
        );
        Ok(())
    }
}

/// The type characters of FIFOs and character and block devices are styled like the others, so
/// coloured rows line up exactly as plain ones do.
#[cfg(unix)]
#[test]
fn long_special_file_types() -> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::fs::{FileTypeExt, symlink};

    let tmp = tempfile::TempDir::new()?;
    let root = tmp.path();
    assert!(
        std::process::Command::new("mkfifo")
            .arg(root.join("fifo"))
            .status()?
            .success()
    );
    symlink("/dev/null", root.join("chr"))?;
    let mut marks = vec!["\x1b[33mp\x1b[0m", "\x1b[1;33mc\x1b[0m"];

    // Containers and the Nix sandbox may expose no block device.
    let block = std::fs::read_dir("/dev")?
        .filter_map(Result::ok)
        .find(|e| e.file_type().is_ok_and(|t| t.is_block_device()));
    if let Some(block) = block {
        symlink(block.path(), root.join("blk"))?;
        marks.push("\x1b[1;33mb\x1b[0m");
    }

    let root = root.to_str().unwrap();
    let plain = utils::Erd::new()
        .args(["--long", "--follow", root])
        .ok()
        .raw();
    let colored = utils::Erd::new()
        .args(["--long", "--follow", "-C", "force", root])
        .ok()
        .raw();

    assert_eq!(
        String::from_utf8(strip_ansi_escapes::strip(&colored))?,
        plain
    );
    for mark in marks {
        assert!(
            colored.lines().any(|l| l.starts_with(mark)),
            "{mark:?} in {colored}"
        );
    }
    Ok(())
}
