use indoc::indoc;

mod utils;

#[test]
fn exclude_directory_removes_size_and_counts() {
    assert_eq!(
        utils::run_cmd(&["--exclude", "lipsum", "tests/data"]),
        indoc!(
            "143 B    ┌─ cassildas_song.md
143 B ┌─ the_yellow_king
100 B ├─ nylarlathotep.txt
161 B ├─ nemesis.txt
 83 B ├─ necronomicon.txt
308 B │  ┌─ polaris.txt
308 B ├─ dream_cycle
795 B data

2 directories, 5 files"
        )
    );
}

#[test]
fn exclude_glob() {
    assert_eq!(
        utils::run_cmd(&["--exclude", "*.txt", "--prune", "tests/data"]),
        indoc!(
            "143 B    ┌─ cassildas_song.md
143 B ┌─ the_yellow_king
143 B data

1 directory, 1 file"
        )
    );
}

#[test]
fn exclude_multiple() {
    assert_eq!(
        utils::run_cmd(&[
            "--exclude",
            "lipsum",
            "--exclude",
            "dream_cycle",
            "tests/data"
        ]),
        indoc!(
            "143 B    ┌─ cassildas_song.md
143 B ┌─ the_yellow_king
100 B ├─ nylarlathotep.txt
161 B ├─ nemesis.txt
 83 B ├─ necronomicon.txt
487 B data

1 directory, 4 files"
        )
    );
}

#[test]
fn exclude_with_pattern() {
    assert_eq!(
        utils::run_cmd(&[
            "--glob",
            "--pattern",
            "*.txt",
            "--exclude",
            "lipsum",
            "tests/data"
        ]),
        indoc!(
            "100 B ┌─ nylarlathotep.txt
161 B ├─ nemesis.txt
 83 B ├─ necronomicon.txt
308 B │  ┌─ polaris.txt
308 B ├─ dream_cycle
652 B data

1 directory, 4 files"
        )
    );
}

#[test]
fn exclude_with_hidden() {
    assert_eq!(
        utils::run_cmd(&["--hidden", "--exclude", "dream_cycle", "tests/data"]),
        indoc!(
            "143 B    ┌─ cassildas_song.md
 143 B ┌─ the_yellow_king
 100 B ├─ nylarlathotep.txt
 161 B ├─ nemesis.txt
  83 B ├─ necronomicon.txt
 446 B │  ┌─ lipsum.txt
 446 B ├─ lipsum
  86 B ├─ .dagon
1019 B data

2 directories, 6 files"
        )
    );
}

#[test]
fn exclude_never_excludes_root() {
    let out = utils::run_cmd(&["--exclude", "data", "tests/data"]);
    assert!(out.ends_with("3 directories, 6 files"), "{out}");
}
