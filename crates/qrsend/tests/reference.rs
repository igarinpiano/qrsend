//! The command reference (docs/CLI.md) explains every command and every
//! option there is: what `--help` shows is looked up in it, so a command or
//! an option added without a word there fails here.

use std::process::Command;

fn help(path: &[String]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_qrsend"))
        .args(path)
        .arg("--help")
        .output()
        .unwrap();
    assert!(out.status.success(), "qrsend {} --help", path.join(" "));
    String::from_utf8(out.stdout).unwrap()
}

/// The subcommands a help text lists (without `help` itself).
fn subcommands(help: &str) -> Vec<String> {
    help.lines()
        .skip_while(|l| *l != "Commands:")
        .skip(1)
        .take_while(|l| !l.is_empty())
        // (A description that runs on is indented further.)
        .filter(|l| l.starts_with("  ") && !l.starts_with("   "))
        .filter_map(|l| l.split_whitespace().next())
        .filter(|name| *name != "help")
        .map(String::from)
        .collect()
}

fn name_end(text: &str) -> usize {
    text.find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
        .unwrap_or(text.len())
}

/// The long options a help text lists (without `--help`).
fn options(help: &str) -> Vec<String> {
    help.lines()
        // (Further in than this, it is a description that mentions an option.)
        .filter(|l| !l.starts_with("       "))
        .filter_map(|l| {
            let l = l.trim_start();
            // "-o, --out <DIR>" or "--out <DIR>".
            let l = match l.as_bytes() {
                [b'-', short, b',', b' ', ..] if *short != b'-' => &l[4..],
                _ => l,
            };
            l.starts_with("--").then(|| l[..name_end(l)].to_string())
        })
        .filter(|option| option != "--help")
        .collect()
}

/// The environment variables a help text names.
fn variables(help: &str) -> Vec<String> {
    help.match_indices("QRSEND_")
        .map(|(at, _)| help[at..at + name_end(&help[at..])].to_string())
        .collect()
}

/// Whether `text` has `word` written as code, and not as the beginning of a
/// longer name (`--export` in `--export-text`).
fn names(text: &str, word: &str) -> bool {
    let written = format!("`{word}");
    text.match_indices(&written)
        .any(|(at, _)| name_end(&text[at + written.len()..]) == 0)
}

/// What the reference says under the heading of a command: up to the heading
/// of the next command.
fn section<'a>(reference: &'a str, command: &str) -> Option<&'a str> {
    let heading = |l: &str| l.starts_with('#') && l.contains("`qrsend ");
    let mut lines = reference.split_inclusive('\n');
    let mut at = 0;
    let start = lines.by_ref().find_map(|l| {
        at += l.len();
        (heading(l) && l.trim_end().ends_with(&format!("`{command}`"))).then_some(at)
    })?;
    let length: usize = lines.take_while(|l| !heading(l)).map(str::len).sum();
    Some(&reference[start..start + length])
}

#[test]
fn the_reference_explains_every_command_and_option() {
    let file = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/CLI.md");
    // (The crate as published comes without the documents.)
    let Ok(reference) = std::fs::read_to_string(file) else {
        return;
    };
    let mut missing = Vec::new();
    let mut seen = 0;
    let mut todo = vec![Vec::<String>::new()];
    while let Some(path) = todo.pop() {
        let text = help(&path);
        let command = ["qrsend".to_string()]
            .iter()
            .chain(&path)
            .cloned()
            .collect::<Vec<_>>()
            .join(" ");
        // (What the program itself takes is explained at the top.)
        let said = match path.is_empty() {
            true => Some(reference.as_str()),
            false => section(&reference, &command),
        };
        let Some(said) = said else {
            missing.push(format!("{command}: no heading"));
            continue;
        };
        for option in options(&text) {
            seen += 1;
            if !names(said, &option) {
                missing.push(format!("{command} {option}"));
            }
        }
        for variable in variables(&text) {
            if !names(&reference, &variable) {
                missing.push(format!("{command}: {variable}"));
            }
        }
        for sub in subcommands(&text) {
            let mut next = path.clone();
            next.push(sub);
            todo.push(next);
        }
    }
    // (Were the help texts read wrongly, nothing would be found missing.)
    assert!(seen > 60, "only {seen} options found in the help texts");
    assert!(
        missing.is_empty(),
        "docs/CLI.md does not explain: {missing:#?}"
    );
}

#[test]
fn help_texts_are_read_as_they_are_written() {
    let text = "Usage: qrsend x <COMMAND>\n\nCommands:\n  list   List\n  clean  Remove what is kept\n         for good\n  help   Print this\n\nOptions:\n  -o, --out <DIR>\n          Where\n      --export-text <FILE>  Write [env: QRSEND_NO_KEEP=]\n  -h, --help  Print help\n";
    assert_eq!(subcommands(text), ["list", "clean"]);
    assert_eq!(options(text), ["--out", "--export-text"]);
    assert_eq!(variables(text), ["QRSEND_NO_KEEP"]);
    let reference = "## `qrsend x`\n`-o`, `--out <DIR>` and `--export-text`\n### `qrsend x list`\n`--all`\n## Else\n";
    assert_eq!(
        section(reference, "qrsend x"),
        Some("`-o`, `--out <DIR>` and `--export-text`\n")
    );
    assert_eq!(
        section(reference, "qrsend x list"),
        Some("`--all`\n## Else\n")
    );
    assert_eq!(section(reference, "qrsend y"), None);
    let said = section(reference, "qrsend x").unwrap();
    assert!(names(said, "--out") && names(said, "--export-text"));
    assert!(!names(said, "--export") && !names(said, "--all"));
}
