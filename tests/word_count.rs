use indoc::indoc;

mod utils;

#[test]
fn word_count() {
    let expected = indoc!(
        " 24    ┌─ cassildas_song.md
          24 ┌─ the_yellow_king
          26 ├─ nylarlathotep.txt
          27 ├─ nemesis.txt
          16 ├─ necronomicon.txt
          69 │  ┌─ lipsum.txt
          69 ├─ lipsum
          56 │  ┌─ polaris.txt
          56 ├─ dream_cycle
         218 data

         3 directories, 6 files"
    );

    // Counts have no unit, so human-readable output is the same.
    for human in [&[][..], &["--human"]] {
        let mut args = vec!["--disk-usage", "word", "tests/data"];
        args.extend_from_slice(human);
        assert_eq!(
            utils::run_cmd_untrimmed(&args).trim_end(),
            expected,
            "{human:?}"
        );
    }
}
