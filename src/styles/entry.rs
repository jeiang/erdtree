//! `LS_COLORS` style of a directory entry, resolved from its [`Stat`] without
//! [`std::fs::Metadata`] or a path. It replicates `LsColors::indicator_for` and
//! `LsColors::style_for` of `lscolors` 0.21 (which are private), using only public API; the
//! tests compare it with `style_for_path_with_metadata` on real files.

use crate::fs::stat::{Kind, Stat};
use lscolors::{Indicator, LsColors, Style};
use std::ffi::OsStr;

/// Resolves entry styles for one [`LsColors`]. Which indicators the user styled is looked up once.
#[derive(Clone, Copy)]
pub struct Resolver {
    ls: &'static LsColors,
    setuid: bool,
    setgid: bool,
    exec: bool,
    multi_link: bool,
    sticky_other_writable: bool,
    other_writable: bool,
    sticky: bool,
    orphan: bool,
}

impl Resolver {
    pub fn new(ls: &'static LsColors) -> Self {
        let has = |indicator| ls.has_explicit_style_for(indicator);

        Self {
            ls,
            setuid: has(Indicator::Setuid),
            setgid: has(Indicator::Setgid),
            exec: has(Indicator::ExecutableFile),
            multi_link: has(Indicator::MultipleHardLinks),
            sticky_other_writable: has(Indicator::StickyAndOtherWritable),
            other_writable: has(Indicator::OtherWritable),
            sticky: has(Indicator::Sticky),
            orphan: has(Indicator::OrphanedSymbolicLink),
        }
    }

    /// The style of the entry `name` with metadata `stat`, as `lscolors` would give for its
    /// path. `is_orphan` tells whether a symlink's target is missing; it is only called for
    /// symlinks when the user styled orphaned links. Metadata of a followed symlink describes
    /// its target, as for `lscolors`.
    pub fn style(
        &self,
        name: &OsStr,
        stat: &Stat,
        is_orphan: impl FnOnce() -> bool,
    ) -> Option<&'static Style> {
        let indicator = self.indicator(stat, is_orphan);

        if indicator == Indicator::RegularFile {
            // Like `lscolors`, a name that is not UTF-8 gets no style at all, not even the
            // style of regular files.
            if let Some(style) = self.ls.style_for_str(name.to_str()?) {
                return Some(style);
            }
        }

        self.ls.style_for_indicator(indicator)
    }

    fn indicator(&self, stat: &Stat, is_orphan: impl FnOnce() -> bool) -> Indicator {
        let mode = stat.mode;

        match stat.kind {
            Kind::File => {
                if self.setuid && mode & 0o4000 != 0 {
                    Indicator::Setuid
                } else if self.setgid && mode & 0o2000 != 0 {
                    Indicator::Setgid
                } else if self.exec && mode & 0o0111 != 0 {
                    Indicator::ExecutableFile
                } else if self.multi_link && stat.nlink > 1 {
                    Indicator::MultipleHardLinks
                } else {
                    Indicator::RegularFile
                }
            },
            Kind::Dir => {
                if self.sticky_other_writable && mode & 0o1002 == 0o1002 {
                    Indicator::StickyAndOtherWritable
                } else if self.other_writable && mode & 0o0002 != 0 {
                    Indicator::OtherWritable
                } else if self.sticky && mode & 0o1000 != 0 {
                    Indicator::Sticky
                } else {
                    Indicator::Directory
                }
            },
            Kind::Symlink => {
                if self.orphan && is_orphan() {
                    Indicator::OrphanedSymbolicLink
                } else {
                    Indicator::SymbolicLink
                }
            },
            #[cfg(unix)]
            Kind::Fifo => Indicator::FIFO,
            #[cfg(unix)]
            Kind::Socket => Indicator::Socket,
            #[cfg(unix)]
            Kind::BlockDevice => Indicator::BlockDevice,
            #[cfg(unix)]
            Kind::CharDevice => Indicator::CharacterDevice,
            Kind::Other => Indicator::MissingFile,
        }
    }
}

#[cfg(all(test, unix))]
mod test {
    use super::*;
    use crate::fs::stat::Fields;
    use std::{
        fs::{self, File},
        os::unix::{
            fs::{PermissionsExt, symlink},
            net::UnixListener,
        },
        path::Path,
        process::Command,
    };

    /// Creates `path` with permission bits `mode`.
    fn file(path: &Path, mode: u32) {
        File::create(path).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    /// Every style the entry at `path` gets from `lscolors` and from the replica must be equal.
    fn check(ls: &'static LsColors, path: &Path) {
        let md = fs::symlink_metadata(path).unwrap();
        let stat = Stat::from_metadata(&md, Fields::default());
        let name = path.file_name().unwrap();

        let expected = ls.style_for_path_with_metadata(path, Some(&md));
        let actual = Resolver::new(ls).style(name, &stat, || !path.exists());

        assert_eq!(
            expected.map(Style::to_nu_ansi_term_style),
            actual.map(Style::to_nu_ansi_term_style),
            "{}",
            path.display()
        );
    }

    /// An `LS_COLORS` that styles every indicator and some suffixes, to reach every branch.
    fn all_styled() -> &'static LsColors {
        Box::leak(Box::new(LsColors::from_string(
            "no=00;37:fi=00;32:di=01;34:ln=01;36:or=40;31;01:pi=33:so=35:bd=33:cd=33:su=37;41:\
             sg=30;43:ex=01;32:mh=44;37:tw=30;42:ow=34;42:st=37;44:*.rs=38;5;208:*.TXT=31",
        )))
    }

    /// Only a few indicators, so the unstyled fallbacks are reached.
    fn few_styled() -> &'static LsColors {
        Box::leak(Box::new(LsColors::from_string(
            "di=01;34:ln=01;36:*.rs=38;5;208",
        )))
    }

    /// An `LS_COLORS` with `fi` unset, so regular files do not fall back to `no`.
    fn no_file_fallback() -> &'static LsColors {
        Box::leak(Box::new(LsColors::from_string("no=00;37:fi=:di=01;34")))
    }

    /// Checks every path under each `LS_COLORS` above and the default one.
    fn check_all(paths: &[std::path::PathBuf]) {
        for ls in [
            all_styled(),
            few_styled(),
            no_file_fallback(),
            Box::leak(Box::new(LsColors::default())),
        ] {
            for path in paths {
                check(ls, path);
            }
        }
    }

    #[test]
    fn matches_lscolors_for_every_kind_of_entry() {
        let dir = tempfile::tempdir().unwrap();
        let p = |name: &str| dir.path().join(name);

        file(&p("plain"), 0o644);
        file(&p("code.rs"), 0o644);
        file(&p("upper.TXT"), 0o644);
        file(&p("exec"), 0o755);
        file(&p("linked"), 0o644);
        fs::hard_link(p("linked"), p("linked2")).unwrap();
        file(&p("exec.rs"), 0o755);

        for (name, mode) in [
            ("dir", 0o755),
            ("dir-sticky", 0o1755),
            ("dir-ow", 0o777),
            ("dir-tw", 0o1777),
            ("dir-ow-only-other", 0o702),
        ] {
            fs::create_dir(p(name)).unwrap();
            fs::set_permissions(p(name), fs::Permissions::from_mode(mode)).unwrap();
        }

        symlink(p("plain"), p("link")).unwrap();
        symlink(p("dir"), p("link-dir")).unwrap();
        symlink(p("missing"), p("dangling")).unwrap();
        symlink(p("link"), p("link-to-link")).unwrap();

        let fifo_ok = Command::new("mkfifo")
            .arg(p("fifo"))
            .status()
            .is_ok_and(|s| s.success());
        assert!(fifo_ok, "mkfifo failed");
        let _socket = UnixListener::bind(p("socket")).unwrap();

        let names: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        assert_eq!(names.len(), 18);

        check_all(&names);

        // Character and block devices.
        for path in ["/dev/null", "/dev/zero"] {
            if Path::new(path).exists() {
                check(all_styled(), Path::new(path));
            }
        }
    }

    /// Separate from the other kinds because the Nix build sandbox refuses setuid and setgid
    /// bits.
    #[test]
    fn setuid_bits_match_ls_colors() {
        let dir = tempfile::tempdir().unwrap();
        let p = |name: &str| dir.path().join(name);

        file(&p("setuid"), 0o4755);
        file(&p("setgid"), 0o2644);
        file(&p("setuid-plain"), 0o4644);

        check_all(&["setuid", "setgid", "setuid-plain"].map(p));
    }

    #[test]
    fn followed_symlink_uses_metadata_of_target() {
        let dir = tempfile::tempdir().unwrap();
        file(&dir.path().join("target.rs"), 0o755);
        symlink(dir.path().join("target.rs"), dir.path().join("link")).unwrap();

        let ls = all_styled();
        let md = fs::metadata(dir.path().join("link")).unwrap();
        let stat = Stat::from_metadata(&md, Fields::default());
        let path = dir.path().join("link");

        assert_eq!(
            ls.style_for_path_with_metadata(&path, Some(&md))
                .map(Style::to_nu_ansi_term_style),
            Resolver::new(ls)
                .style(path.file_name().unwrap(), &stat, || false)
                .map(Style::to_nu_ansi_term_style)
        );
    }

    // APFS and other UTF-8-only file systems refuse such names (EILSEQ).
    #[cfg(target_os = "linux")]
    #[test]
    fn invalid_utf8_name_gets_no_regular_file_style() {
        use std::os::unix::ffi::OsStrExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(OsStr::from_bytes(b"bad\xff.rs"));
        file(&path, 0o644);

        check(all_styled(), &path);
        assert!(
            Resolver::new(all_styled())
                .style(
                    path.file_name().unwrap(),
                    &Stat::from_metadata(&fs::symlink_metadata(&path).unwrap(), Fields::default()),
                    || false
                )
                .is_none()
        );
    }
}
