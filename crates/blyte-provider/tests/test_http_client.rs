//! One HTTP client per process, so model calls reuse connections instead of a TCP and TLS
//! handshake each (15-45 ms per call).

#[test]
fn the_http_client_is_shared() {
    let a = blyte_provider::http_client() as *const ureq::Agent;
    let b = blyte_provider::http_client() as *const ureq::Agent;
    assert_eq!(a, b);
}
