mod utils;

#[test]
fn no_report_omits_summary_and_trailing_blank_line() {
    for layout in ["regular", "inverted", "flat", "iflat"] {
        let with = utils::run_cmd_untrimmed(&["--layout", layout, "tests/data"]);
        let without = utils::run_cmd_untrimmed(&["--layout", layout, "--no-report", "tests/data"]);

        // The rows are unchanged; only the blank line and the summary go away.
        let (rows, summary) = with
            .split_once("\n\n")
            .expect("summary separated by a blank line");
        assert_eq!(summary, "3 directories, 6 files\n", "{layout}");
        assert_eq!(without, format!("{rows}\n"), "{layout}");
    }
}
