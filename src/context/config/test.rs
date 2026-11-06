use super::{error::Error, parse};
use config::{Config, File, FileFormat};
use std::collections::BTreeMap;

const TOML: &str = r#"
    icons = true
    human = false
    threads = 10
    disk_usage = "block"
    exclude = ["b*", "a*"]

    [grogoroth]
    dir_order = "first"
    icons = false
    hidden = true
    level = 2
"#;

fn parse_str(toml: &str, table: Option<&str>) -> Result<Vec<String>, Error> {
    let config = Config::builder()
        .add_source(File::from_str(toml, FileFormat::Toml))
        .build()?;
    Ok(parse(config, table)?
        .into_iter()
        .map(|arg| arg.into_string().unwrap())
        .collect())
}

/// The arguments by flag: the values of each occurrence, in order. Flags come out in the order
/// of a hash map, so only their order among themselves is ignored.
fn by_flag(args: &[String]) -> BTreeMap<&str, Vec<Vec<&str>>> {
    let mut flags: BTreeMap<&str, Vec<Vec<&str>>> = BTreeMap::new();
    let mut current = None;
    for arg in args {
        if arg.starts_with("--") {
            flags.entry(arg).or_default().push(vec![]);
            current = Some(arg.as_str());
        } else {
            let flag = current.expect("a value follows its flag");
            flags.get_mut(flag).unwrap().last_mut().unwrap().push(arg);
        }
    }
    flags
}

fn expected<'a>(pairs: &[(&'a str, &[&'a str])]) -> BTreeMap<&'a str, Vec<Vec<&'a str>>> {
    let mut flags: BTreeMap<&str, Vec<Vec<&str>>> = BTreeMap::new();
    for (flag, values) in pairs {
        flags.entry(flag).or_default().push(values.to_vec());
    }
    flags
}

#[test]
fn top_level_table_becomes_arguments() {
    let args = parse_str(TOML, None).unwrap();
    assert_eq!(
        by_flag(&args),
        expected(&[
            ("--icons", &[]),
            ("--threads", &["10"]),
            ("--disk-usage", &["block"]),
            ("--exclude", &["b*"]),
            ("--exclude", &["a*"]),
        ])
    );
}

#[test]
fn named_table_replaces_top_level() {
    let args = parse_str(TOML, Some("grogoroth")).unwrap();
    assert_eq!(
        by_flag(&args),
        expected(&[
            ("--dir-order", &["first"]),
            ("--hidden", &[]),
            ("--level", &["2"]),
        ])
    );
}

#[test]
fn invalid_values_are_errors() {
    assert!(matches!(
        parse_str(TOML, Some("missing")),
        Err(Error::MissingAltConfig(table)) if table == "missing"
    ));
    assert!(matches!(
        parse_str("level = -1", None),
        Err(Error::InvalidInteger(key)) if key == "level"
    ));
    assert!(matches!(
        parse_str("level = 1.5", None),
        Err(Error::InvalidArgument(key)) if key == "level"
    ));
    assert!(matches!(
        parse_str("exclude = [\"a\", 1]", None),
        Err(Error::InvalidArgument(key)) if key == "exclude"
    ));
}
