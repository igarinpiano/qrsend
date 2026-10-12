//! What the system's firewall does with connections coming in to this
//! program. `send --lan` waits for the receiver to connect, and a firewall
//! that turns the receiver away does so without a word to either side: the
//! codes go on, and no connection ever comes about. So the sender asks
//! beforehand, where the system can be asked (macOS).

/// What becomes of a connection coming in to this program.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Incoming {
    /// Nothing stands in the way, as far as can be told.
    Open,
    /// The firewall turns it away.
    Blocked,
    /// The firewall has no rule for this program yet: the system asks the
    /// user, and turns connections away until the answer is to allow them.
    Asked,
}

impl Incoming {
    /// The word for the log.
    pub fn word(self) -> &'static str {
        match self {
            Incoming::Open => "open",
            Incoming::Blocked => "blocked",
            Incoming::Asked => "asked",
        }
    }
}

/// The firewall's own tool, which answers without special rights.
const TOOL: &str = "/usr/libexec/ApplicationFirewall/socketfilterfw";

/// What the firewall does with connections to this program, and the path it
/// knows the program by. None: it cannot be told (no such tool, or a system
/// other than macOS).
pub fn incoming() -> Option<(Incoming, String)> {
    #[cfg(target_os = "macos")]
    {
        let ask = |args: &[&str]| -> Option<String> {
            let out = std::process::Command::new(TOOL)
                .args(args)
                .stdin(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .output()
                .ok()?;
            Some(String::from_utf8_lossy(&out.stdout).into_owned())
        };
        let program = std::env::current_exe().ok()?;
        let program = program.canonicalize().unwrap_or(program);
        let program = program.to_str()?.to_string();
        let state = judge(
            &ask(&["--getglobalstate"])?,
            &ask(&["--getappblocked", &program])?,
            &ask(&["--listapps"])?,
            &program,
        )?;
        Some((state, program))
    }
    #[cfg(not(target_os = "macos"))]
    None
}

/// Reads the tool's three answers: the firewall's state, whether `program`
/// is blocked, and the list of programs with a rule.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn judge(state: &str, blocked: &str, listed: &str, program: &str) -> Option<Incoming> {
    // "Firewall is enabled. (State = 1)"; 0 is off, 2 blocks everything.
    let state: u8 = state
        .split("State = ")
        .nth(1)?
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .ok()?;
    if state == 0 {
        return Some(Incoming::Open);
    }
    // "Incoming connection to <path> is blocked." / "… is permitted."
    if state == 2 || blocked.contains("is blocked") {
        return Some(Incoming::Blocked);
    }
    // "N : <path> " and, on the next line, "( Allow incoming connections )".
    // A program without a rule is "permitted" above, and yet nothing reaches
    // it until the user has answered the system's question.
    let mut lines = listed.lines();
    while let Some(line) = lines.next() {
        if line
            .split_once(" : ")
            .is_some_and(|(_, path)| path.trim() == program)
        {
            return Some(match lines.next() {
                Some(rule) if rule.contains("Allow") => Incoming::Open,
                _ => Incoming::Blocked,
            });
        }
    }
    Some(Incoming::Asked)
}

/// What to tell someone who offers a connection that the firewall will turn
/// away; nothing when nothing stands in the way.
pub fn advice(state: Incoming, program: &str) -> Option<String> {
    match state {
        Incoming::Open => None,
        Incoming::Blocked => Some(format!(
            "The macOS firewall turns away connections to this program, so no receiver can \
             connect over the network (the codes on the screen work as ever). To allow them:\n{}",
            manual(program)
        )),
        Incoming::Asked => Some(format!(
            "The macOS firewall has no rule yet for this program: it may ask whether \"qrsend\" \
             may accept incoming connections (choose Allow), and until then no receiver can \
             connect over the network. Or beforehand:\n{}",
            manual(program)
        )),
    }
}

/// Sees to it, as far as this program can, that the firewall lets a
/// receiver in. Changing the firewall's rules takes an administrator's
/// rights, so nothing happens by itself: on a terminal the user is asked,
/// and with a yes the firewall's own tool is run through `sudo` (which asks
/// for the password); otherwise what to do is printed. Returns what the
/// firewall does with incoming connections afterwards.
pub fn settle(state: Incoming, program: &str) -> Incoming {
    use std::io::{BufRead, IsTerminal, Write};
    let Some(advice) = advice(state, program) else {
        return state;
    };
    if !cfg!(target_os = "macos")
        || !std::io::stdin().is_terminal()
        || !std::io::stderr().is_terminal()
    {
        eprintln!("{advice}");
        return state;
    }
    eprintln!(
        "The macOS firewall {} connections to this program, so no receiver could connect over \
         the network (the codes on the screen work as ever).",
        match state {
            Incoming::Blocked => "turns away",
            _ => "has no rule yet for",
        }
    );
    eprint!(
        "Allow them now? This changes the firewall's rule for qrsend and takes an administrator's password (sudo). [y/N] "
    );
    let _ = std::io::stderr().flush();
    let mut answer = String::new();
    let _ = std::io::stdin().lock().read_line(&mut answer);
    if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        eprintln!("Left as it is. To allow them later:\n{}", manual(program));
        return state;
    }
    // (A program without a rule is added first; one with a rule only has
    // its rule changed.)
    for action in ["--add", "--unblockapp"] {
        let _ = std::process::Command::new("sudo")
            .args([TOOL, action, program])
            .stdout(std::process::Stdio::null())
            .status();
    }
    match incoming() {
        Some((Incoming::Open, _)) => {
            eprintln!("Allowed.");
            Incoming::Open
        }
        other => {
            eprintln!(
                "The firewall still turns connections away. By hand:\n{}",
                manual(program)
            );
            other.map_or(state, |(state, _)| state)
        }
    }
}

/// How to allow incoming connections by hand.
fn manual(program: &str) -> String {
    format!(
        "  System Settings → Network → Firewall → Options…, and set \"qrsend\" to \"Allow incoming connections\", or\n  \
         sudo {TOOL} --add '{program}' && sudo {TOOL} --unblockapp '{program}'"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const LISTED: &str = "Total number of apps = 3 \n\
        1 : /opt/node/bin/qrsend \n             (Block incoming connections) \n\n\
        2 : /usr/bin/python3 \n             (Allow incoming connections) \n\n\
        3 : /opt/node/bin/qrsend-old \n             (Allow incoming connections) \n";
    const ON: &str = "Firewall is enabled. (State = 1) \n";

    #[test]
    fn the_firewalls_answers_are_read() {
        let permitted = "Incoming connection to X is permitted. \n";
        let blocked = "Incoming connection to X is blocked. \n";
        // Off: nothing is turned away, whatever the list says.
        assert_eq!(
            judge(
                "Firewall is disabled. (State = 0)",
                blocked,
                LISTED,
                "/opt/node/bin/qrsend"
            ),
            Some(Incoming::Open)
        );
        assert_eq!(
            judge(ON, blocked, LISTED, "/opt/node/bin/qrsend"),
            Some(Incoming::Blocked)
        );
        // (The list alone says so too.)
        assert_eq!(
            judge(ON, permitted, LISTED, "/opt/node/bin/qrsend"),
            Some(Incoming::Blocked)
        );
        assert_eq!(
            judge(ON, permitted, LISTED, "/usr/bin/python3"),
            Some(Incoming::Open)
        );
        // A name that another begins with is not that other.
        assert_eq!(
            judge(ON, permitted, LISTED, "/opt/node/bin/qrsend-old"),
            Some(Incoming::Open)
        );
        // No rule yet: the system will ask.
        assert_eq!(
            judge(ON, permitted, LISTED, "/somewhere/else/qrsend"),
            Some(Incoming::Asked)
        );
        // Everything blocked.
        assert_eq!(
            judge(
                "Firewall is on. (State = 2)",
                permitted,
                LISTED,
                "/usr/bin/python3"
            ),
            Some(Incoming::Blocked)
        );
        // An answer that is not understood is no answer.
        assert_eq!(judge("", permitted, LISTED, "/usr/bin/python3"), None);
        assert_eq!(
            judge("State = x", permitted, LISTED, "/usr/bin/python3"),
            None
        );
    }

    #[test]
    fn advice_only_when_something_is_in_the_way() {
        assert!(advice(Incoming::Open, "/p/qrsend").is_none());
        assert!(
            advice(Incoming::Blocked, "/p/qrsend")
                .unwrap()
                .contains("--unblockapp '/p/qrsend'")
        );
        assert!(
            advice(Incoming::Asked, "/p/qrsend")
                .unwrap()
                .contains("Allow")
        );
    }
}
