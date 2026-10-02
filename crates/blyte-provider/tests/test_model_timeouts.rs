use std::time::Duration;

#[test]
fn default_timeouts() {
    let (whole, read) = blyte_provider::model_timeouts_from(None);
    assert_eq!(whole, Duration::from_secs(180));
    assert_eq!(read, Duration::from_secs(150));

    // At least 30s read timeout
    let (whole40, read40) = blyte_provider::model_timeouts_from(Some("40"));
    assert_eq!(whole40, Duration::from_secs(40));
    assert_eq!(read40, Duration::from_secs(30));

    let client = blyte_provider::build_http_client(whole, read);
    drop(client);
}

#[test]
fn env_override_gives_60_and_30() {
    unsafe {
        std::env::set_var("BLYTE_MODEL_TIMEOUT_SECS", "60");
    }
    let (whole, read) = blyte_provider::model_timeouts();
    assert_eq!(whole, Duration::from_secs(60));
    assert_eq!(read, Duration::from_secs(30));
    let client = blyte_provider::http_client();
    let _ = client;
}
