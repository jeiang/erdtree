use indoc::indoc;

mod utils;

#[test]
fn level() {
    assert_eq!(
        utils::run_cmd(&["--level", "1", "tests/data"]),
        indoc!(
            "143 B ┌─ the_yellow_king
 100 B ├─ nylarlathotep.txt
 161 B ├─ nemesis.txt
  83 B ├─ necronomicon.txt
 446 B ├─ lipsum
 308 B ├─ dream_cycle
1241 B data

3 directories, 6 files"
        )
    )
}

/// The summary counts every entry, whatever the level cuts off the display.
#[test]
fn level_zero_counts_everything() {
    assert_eq!(
        utils::run_cmd(&["--level", "0", "tests/data"]),
        indoc!(
            "1241 B data

3 directories, 6 files"
        )
    );
}

/// A directory at the level has no children shown, but the ones above keep their branches.
#[test]
fn level_two_regular() {
    let (_tmp, root) = utils::three_levels();
    assert_eq!(
        utils::run_cmd(&["--level", "2", &root]),
        indoc!(
            "4 B ┌─ e.txt
            2 B │  ┌─ d.txt
            3 B │  ├─ b
            5 B ├─ a
            9 B t

            2 directories, 3 files"
        )
    );
}

#[test]
fn level_one_inverted() {
    assert_eq!(
        utils::run_cmd(&["--level", "1", "--layout", "inverted", "tests/data"]),
        indoc!(
            "1241 B data
 308 B ├─ dream_cycle
 446 B ├─ lipsum
  83 B ├─ necronomicon.txt
 161 B ├─ nemesis.txt
 100 B ├─ nylarlathotep.txt
 143 B └─ the_yellow_king

3 directories, 6 files"
        )
    );
}

#[test]
fn level_two_inverted() {
    let (_tmp, root) = utils::three_levels();
    assert_eq!(
        utils::run_cmd(&["--level", "2", "--layout", "inverted", &root]),
        indoc!(
            "9 B t
            5 B ├─ a
            3 B │  ├─ b
            2 B │  └─ d.txt
            4 B └─ e.txt

            2 directories, 3 files"
        )
    );
}

/// Flat layouts sort only what the level shows, in the same order as sorting everything: by
/// size across directories, so `e.txt` comes before `a/b`.
#[test]
fn level_two_flat_by_size() {
    let (_tmp, root) = utils::three_levels();
    assert_eq!(
        utils::run_cmd(&["--level", "2", "--layout", "iflat", "--sort", "size", &root]),
        indoc!(
            "9 B   t
            5 B   a
            4 B   e.txt
            3 B   a/b
            2 B   a/d.txt

            2 directories, 3 files"
        )
    );
}
