use std::path::PathBuf;
use std::process::Command;

fn axon_bin() -> PathBuf {
    if let Some(bin) = std::env::var_os("CARGO_BIN_EXE_axon") {
        return PathBuf::from(bin);
    }

    let current = std::env::current_exe().expect("resolve current test executable");
    let debug_dir = current
        .parent()
        .and_then(std::path::Path::parent)
        .expect("resolve target debug dir");
    let fallback = if cfg!(windows) {
        debug_dir.join("axon.exe")
    } else {
        debug_dir.join("axon")
    };
    assert!(
        fallback.exists(),
        "failed to locate axon binary via CARGO_BIN_EXE_axon and fallback path {}",
        fallback.display()
    );
    fallback
}

#[test]
fn spec_cli_commands_are_present_in_help_output() {
    // SPEC.md §6 command inventory (at minimum these commands must exist).
    let expected = [
        "daemon", "send", "notify", "peers", "status", "identity", "whoami", "doctor", "examples",
    ];

    let output = Command::new(axon_bin())
        .arg("--help")
        .output()
        .expect("failed running axon --help");
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);

    for command in expected {
        assert!(
            help.contains(&format!("  {command}")),
            "command '{command}' listed in spec should appear in `axon --help`"
        );
    }

    assert!(
        help.contains("--state-root"),
        "global state-root flag documented in spec should appear in `axon --help`"
    );
}
