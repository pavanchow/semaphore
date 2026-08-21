use semaphore::{serve_on_ephemeral, Client};
use std::time::Duration;

#[test]
fn set_get_ping_over_real_tcp() {
    let (addr, _handle) = serve_on_ephemeral("127.0.0.1:0").expect("bind ephemeral port");
    std::thread::sleep(Duration::from_millis(50));

    let mut client = Client::connect(addr).expect("connect");

    client.ping().expect("ping");

    client.set("alpha", b"beta").expect("set");
    let value = client.get("alpha").expect("get");
    assert_eq!(value, Some(b"beta".to_vec()));

    let missing = client.get("does-not-exist").expect("get missing");
    assert_eq!(missing, None);
}
