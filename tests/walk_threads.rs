//! The multi-threaded walk, which splits directories above 1024 entries between workers, must
//! show exactly what a single thread shows.

use std::fs;
use utils::Erd;

mod utils;

fn run(threads: usize, dir: &std::path::Path, extra: &[&str]) -> String {
    Erd::new()
        .threads(threads)
        .args(extra)
        .arg(dir)
        .ok()
        .stdout()
}

#[test]
fn many_threads_match_one_thread_on_large_directories() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    // Two directories well above the split threshold, one of them holding subdirectories.
    for (dir, files) in [("wide", 2600), ("wider", 5000)] {
        fs::create_dir(root.join(dir)).unwrap();
        for i in 0..files {
            let name = if i % 11 == 0 {
                format!(".dot-{i}")
            } else {
                format!("f{i}")
            };
            fs::write(root.join(dir).join(name), "x".repeat(i % 29)).unwrap();
        }
    }
    for d in 0..30 {
        let sub = root.join("wide").join(format!("sub{d}"));
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("inner"), "i".repeat(d)).unwrap();
    }
    fs::write(root.join("wide").join("hard"), "hard link").unwrap();
    fs::hard_link(
        root.join("wide").join("hard"),
        root.join("wider").join("hard2"),
    )
    .unwrap();

    let mut cases = vec![
        &[][..],
        &["--report-ignored"],
        &["-H", "-y", "flat"],
        &["--min-size", "10"],
    ];
    // `--disk-usage block` and the long view exist only on Unix.
    if cfg!(unix) {
        cases.push(&["-d", "block", "-l"]);
    }

    for extra in cases {
        let one = run(1, root, extra);

        for _ in 0..3 {
            assert_eq!(run(4, root, extra), one, "{extra:?}");
        }
    }
}
