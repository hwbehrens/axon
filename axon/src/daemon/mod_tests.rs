use super::*;

#[test]
fn daemon_options_default() {
    let opts = DaemonOptions::default();
    assert!(opts.port.is_none());
    assert!(!opts.disable_mdns);
    assert!(opts.axon_root.is_none());
}
