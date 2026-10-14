mod utils;

#[test]
fn summarize_prints_only_the_root_total() {
    assert_eq!(
        utils::run_cmd(&["--summarize", "tests/data"]),
        "1241 B data"
    );

    for layout in ["regular", "inverted", "flat", "iflat"] {
        let summarized =
            utils::run_cmd_untrimmed(&["--summarize", "--layout", layout, "tests/data"]);
        assert_eq!(summarized.lines().count(), 1, "{layout}: {summarized:?}");

        // `--summarize` wins over an explicit `--level`.
        let with_level = utils::run_cmd_untrimmed(&[
            "--summarize",
            "--level",
            "2",
            "--layout",
            layout,
            "tests/data",
        ]);
        assert_eq!(with_level, summarized, "{layout}");
    }
}
