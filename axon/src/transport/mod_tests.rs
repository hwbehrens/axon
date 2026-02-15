use super::*;

#[test]
fn transport_constants() {
    assert_eq!(REQUEST_TIMEOUT, Duration::from_secs(30));
}
