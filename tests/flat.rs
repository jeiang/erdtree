use indoc::indoc;

mod utils;

#[test]
fn flat() {
    assert_eq!(
        utils::run_cmd(&["--layout", "flat", "tests/data"]),
        indoc!(
            "143 B   the_yellow_king
 308 B   dream_cycle/polaris.txt
 100 B   nylarlathotep.txt
 161 B   nemesis.txt
  83 B   necronomicon.txt
 446 B   lipsum/lipsum.txt
 446 B   lipsum
 308 B   dream_cycle
 143 B   the_yellow_king/cassildas_song.md
1241 B   data

3 directories, 6 files"
        )
    )
}

#[test]
fn flat_human() {
    assert_eq!(
        utils::run_cmd(&["--layout", "flat", "--human", "tests/data"]),
        indoc!(
            "143   B   the_yellow_king
 308   B   dream_cycle/polaris.txt
 100   B   nylarlathotep.txt
 161   B   nemesis.txt
  83   B   necronomicon.txt
 446   B   lipsum/lipsum.txt
 446   B   lipsum
 308   B   dream_cycle
 143   B   the_yellow_king/cassildas_song.md
 1.2 KiB   data
 
 3 directories, 6 files"
        )
    )
}

#[test]
fn flat_with_level() {
    assert_eq!(
        utils::run_cmd(&["--layout", "flat", "--level", "1", "tests/data"]),
        indoc!(
            "143 B   the_yellow_king
 100 B   nylarlathotep.txt
 161 B   nemesis.txt
  83 B   necronomicon.txt
 446 B   lipsum
 308 B   dream_cycle
1241 B   data

3 directories, 6 files"
        )
    )
}

/// Flat layouts order every entry against every other entry, not per directory. In `tests/flat_data`
/// `a` (60 B) contains `c` (40 B), which is smaller than its sibling `b` (50 B).
#[test]
fn flat_dirs_only_sorts_globally_by_size() {
    assert_eq!(
        utils::run_cmd(&[
            "--layout",
            "flat",
            "--sort",
            "size",
            "--dirs-only",
            "tests/flat_data"
        ]),
        indoc!(
            "40 B   a/c
 50 B   b
 60 B   a
110 B   flat_data

3 directories"
        )
    )
}

#[test]
fn iflat_dirs_only_sorts_globally_by_size() {
    assert_eq!(
        utils::run_cmd(&[
            "--layout",
            "iflat",
            "--sort",
            "size",
            "--dirs-only",
            "tests/flat_data"
        ]),
        indoc!(
            "110 B   flat_data
 60 B   a
 50 B   b
 40 B   a/c

3 directories"
        )
    )
}

#[test]
fn iflat_sorts_files_and_dirs_globally_by_size() {
    assert_eq!(
        utils::run_cmd(&["--layout", "iflat", "--sort", "size", "tests/flat_data"]),
        indoc!(
            "110 B   flat_data
 60 B   a
 50 B   b
 50 B   b/b1.txt
 40 B   a/c
 40 B   a/c/c1.txt
 20 B   a/a1.txt

3 directories, 3 files"
        )
    )
}

/// A suppressed size column must not leave leading whitespace or a blank column behind.
#[test]
fn flat_suppress_size_has_no_extra_whitespace() {
    for layout in ["flat", "iflat"] {
        let out = utils::run_cmd_untrimmed(&[
            "--layout",
            layout,
            "--suppress-size",
            "--sort",
            "name",
            "tests/flat_data",
        ]);
        let rows = out
            .lines()
            .take_while(|l| !l.is_empty())
            .collect::<Vec<_>>();

        assert_eq!(rows.len(), 7, "{layout}: {out:?}");
        for row in rows {
            assert_eq!(row, row.trim(), "{layout}: whitespace in {row:?}");
            assert!(!row.contains("  "), "{layout}: blank column in {row:?}");
        }
    }
}
