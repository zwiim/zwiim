//! LAN access end to end over real TCP sockets: the operator's switch, the
//! listener serving the device tunnel, device trust and revocation, and the
//! mDNS advertisement.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use super::{service_info, Settings, SERVICE_TYPE};
use crate::identity::device::DeviceIdentity;
use crate::identity::host_keys::HostKeys;
use crate::identity::pairing::{KeyPairingGrant, KeyPairingInvitation};
use crate::protocol::tests::{host, Host, HOST_TOKEN};
use crate::tunnel::client::TunnelClient;
use crate::tunnel::noise::TunnelError;
use crate::tunnel::tests::{call, closed, direct, events, frame, new_device, ok, paired_device, paired_device_at, WAIT};
use crate::web::shutdown::ShutdownSignal;

/// A host with its LAN access runtime started (switched off), not
/// advertising unless `advertise`.
async fn lan_host(advertise: bool) -> (Host, Arc<ShutdownSignal>) {
    let h = host(false).await;
    h.state.lan.configure(Settings { port: 0, advertise });
    let shutdown = Arc::new(ShutdownSignal::new());
    let router = crate::tunnel::host::router(h.state.clone(), HOST_TOKEN.to_string(), shutdown.clone());
    super::start(h.state.clone(), router, shutdown.clone());
    (h, shutdown)
}

/// Turn LAN access on; the port it listens on.
async fn enable(h: &Host) -> u16 {
    let status = h.ok("lan.set", json!({"enabled": true})).await;
    assert_eq!(status["enabled"], true, "{status}");
    assert_eq!(status["listening"], true, "{status}");
    status["port"].as_u64().unwrap() as u16
}

async fn dial(addr: SocketAddr) -> TcpStream {
    tokio::time::timeout(WAIT, TcpStream::connect(addr)).await.expect("connect within 5s").unwrap()
}

/// A device tunnel over a fresh connection. On macOS, a new test binary's
/// incoming connections on a network address wait for the application
/// firewall's prompt: that fails here rather than hangs.
async fn tunnel(device: &DeviceIdentity, host_id: &str, addr: SocketAddr) -> TunnelClient {
    let stream = dial(addr).await;
    let noise = tokio::time::timeout(WAIT, device.connect_tunnel(stream, host_id))
        .await
        .unwrap_or_else(|_| panic!("no handshake over {addr} within 5s (is this test binary allowed by the firewall?)"));
    TunnelClient::start(noise.unwrap())
}

fn loopback(port: u16) -> SocketAddr {
    (Ipv4Addr::LOCALHOST, port).into()
}

/// A non-loopback IPv4 address of this machine, as another device on the
/// network would reach it.
fn lan_address() -> Option<Ipv4Addr> {
    if_addrs::get_if_addrs().ok()?.into_iter().find_map(|i| match i.ip() {
        IpAddr::V4(ip) if !ip.is_loopback() && !ip.is_link_local() => Some(ip),
        _ => None,
    })
}

/// The peer closed without sending a byte.
async fn closed_silently(mut stream: TcpStream) {
    let mut buf = [0u8; 64];
    let n = tokio::time::timeout(WAIT, stream.read(&mut buf)).await.expect("closed within 5s").unwrap_or(0);
    assert_eq!(n, 0, "the host answered: {:?}", String::from_utf8_lossy(&buf[..n]));
}

#[tokio::test]
async fn lan_access_is_off_until_the_operator_turns_it_on() {
    let (h, _shutdown) = lan_host(false).await;
    assert_eq!(
        h.ok("lan.status", json!({})).await,
        json!({"enabled": false, "listening": false, "port": null, "advertised": false, "last_error": null})
    );
    let port = enable(&h).await;
    assert_eq!(h.ok("lan.status", json!({})).await["port"], port);
    assert_eq!(super::config(&h.state.db.conn).await.unwrap().enabled, true);

    // Operator only: no credential, a wrong one, or bad input are refused.
    let res = h.server.post("/zwiim/v1/lan.set").json(&json!({"enabled": false})).await;
    assert_eq!(res.status_code().as_u16(), 401);
    let (status, body) = direct(&h, "lan.set", json!({"enabled": "yes"})).await;
    assert_eq!((status, body["error"]["code"].as_str()), (400, Some("invalid_request")), "{body}");

    let status = h.ok("lan.set", json!({"enabled": false})).await;
    assert_eq!((status["enabled"].clone(), status["listening"].clone()), (json!(false), json!(false)));
    let refused = TcpStream::connect(loopback(port)).await;
    assert!(refused.is_err(), "still listening on {port}");
}

#[tokio::test]
async fn a_host_that_was_left_on_starts_listening_by_itself() {
    let h = host(false).await;
    h.state.lan.configure(Settings { port: 0, advertise: false });
    // Stored before the runtime exists, as by an earlier run of the host.
    let status = h.ok("lan.set", json!({"enabled": true})).await;
    assert_eq!((status["enabled"].clone(), status["listening"].clone()), (json!(true), json!(false)));

    let shutdown = Arc::new(ShutdownSignal::new());
    let router = crate::tunnel::host::router(h.state.clone(), HOST_TOKEN.to_string(), shutdown.clone());
    super::start(h.state.clone(), router, shutdown.clone());
    let deadline = tokio::time::Instant::now() + WAIT;
    while h.state.lan.listening().port.is_none() {
        assert!(tokio::time::Instant::now() < deadline, "not listening");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let (device, host_id) = paired_device(&h).await;
    let c = tunnel(&device, &host_id, loopback(h.state.lan.listening().port.unwrap())).await;
    ok(&h, &c, "host.hello", json!({})).await;

    // Host shutdown ends LAN tunnels and the listener.
    shutdown.trigger();
    tokio::time::timeout(WAIT, async {
        while !c.is_closed() || h.state.lan.listening().port.is_some() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("stopped");
}

#[tokio::test]
async fn paired_devices_use_the_host_over_the_lan_port() {
    let (h, _shutdown) = lan_host(false).await;
    let port = enable(&h).await;
    let (device, host_id) = paired_device(&h).await;

    let mut addrs = vec![loopback(port)];
    // The network address too, when this machine has one.
    addrs.extend(lan_address().map(|ip| SocketAddr::from((ip, port))));
    for addr in addrs {
        let c = tunnel(&device, &host_id, addr).await;
        assert_eq!(ok(&h, &c, "host.hello", json!({})).await["host_id"], host_id.as_str(), "{addr}");
        for (op, params) in [
            ("workspace.list", json!({})),
            ("git.status", json!({"workspace_id": h.workspace_id})),
            ("files.read", json!({"workspace_id": h.workspace_id, "path": "nope.txt"})),
        ] {
            let over_lan = call(&h, &c, op, params.clone()).await;
            assert_eq!(over_lan, direct(&h, op, params).await, "{op} over {addr}");
        }
        let mut stream = events(&c).await;
        stream.send(&json!({"op": "host.subscribe"})).unwrap();
        assert_eq!(frame(&h, &mut stream).await["type"], "host.snapshot");

        // Still a device, not the operator; nothing but `/zwiim/v1`.
        for op in ["lan.set", "lan.status", "device.list", "cloud.status"] {
            let (status, body) = call(&h, &c, op, json!({"enabled": true})).await;
            assert_eq!((status, body["error"]["code"].as_str()), (403, Some("permission_denied")), "{op}");
        }
        for op in ["../../api/list_folders", "../api/trusted_device_list"] {
            let (status, body) = call(&h, &c, op, json!({})).await;
            assert_eq!((status, body["error"]["code"].as_str()), (404, Some("unknown_operation")), "{op}");
        }
        c.close().await;
    }
}

#[tokio::test]
async fn unknown_revoked_and_misled_devices_are_refused_on_the_lan() {
    let (h, _shutdown) = lan_host(false).await;
    let addr = loopback(enable(&h).await);
    let invitation: KeyPairingInvitation =
        serde_json::from_value(h.ok("device.pair_invite", json!({})).await).unwrap();
    let host_id = invitation.host_id.clone();

    // Knows the host's key and is on the network, but never paired.
    let (mut stranger, path) = new_device(&h);
    let grant = KeyPairingGrant { host_id: host_id.clone(), device_id: stranger.device_id.clone() };
    stranger.record_key_pairing(&path, &invitation, &grant).unwrap();
    assert!(matches!(stranger.connect_tunnel(dial(addr).await, &host_id).await, Err(TunnelError::Refused)));

    // Revoked.
    let (device, _) = paired_device(&h).await;
    h.ok("device.revoke", json!({"device_id": device.device_id})).await;
    assert!(matches!(device.connect_tunnel(dial(addr).await, &host_id).await, Err(TunnelError::Refused)));

    // Pinned another key (a host impersonating this one): no answer at all.
    let (mut misled, path, _) = paired_device_at(&h).await;
    let wrong = KeyPairingInvitation { host_static_pubkey: HostKeys::generate().static_pubkey(), ..invitation };
    let grant = KeyPairingGrant { host_id: host_id.clone(), device_id: misled.device_id.clone() };
    misled.record_key_pairing(&path, &wrong, &grant).unwrap();
    assert!(matches!(misled.connect_tunnel(dial(addr).await, &host_id).await, Err(TunnelError::Handshake)));

    let devices = h.ok("device.list", json!({})).await["devices"].clone();
    let seen = devices.as_array().unwrap().iter().filter(|d| !d["last_seen_at"].is_null()).count();
    assert_eq!(seen, 0, "{devices}");
}

#[tokio::test]
async fn revoking_a_device_ends_its_lan_tunnel() {
    let (h, _shutdown) = lan_host(false).await;
    let addr = loopback(enable(&h).await);
    let (device, host_id) = paired_device(&h).await;
    let c = tunnel(&device, &host_id, addr).await;
    let mut stream = events(&c).await;
    stream.send(&json!({"op": "host.subscribe"})).unwrap();
    assert_eq!(frame(&h, &mut stream).await["type"], "host.snapshot");

    h.ok("device.revoke", json!({"device_id": device.device_id})).await;
    assert_eq!(closed(&mut stream).await, (Some(4403), Some("device_revoked".into())));
    assert!(matches!(c.request("host.hello", json!({})).await, Err(TunnelError::Closed)));
    assert!(matches!(device.connect_tunnel(dial(addr).await, &host_id).await, Err(TunnelError::Refused)));
}

#[tokio::test]
async fn the_lan_port_speaks_only_the_tunnel() {
    let (h, _shutdown) = lan_host(false).await;
    let addr = loopback(enable(&h).await);
    for probe in [
        &b"GET /api/list_folders HTTP/1.1\r\nHost: x\r\n\r\n"[..],
        b"POST /zwiim/v1/host.hello HTTP/1.1\r\nAuthorization: Bearer host-token\r\nContent-Length: 2\r\n\r\n{}",
        b"\x00\x05hello",
    ] {
        let mut stream = dial(addr).await;
        stream.write_all(probe).await.unwrap();
        stream.shutdown().await.unwrap();
        closed_silently(stream).await;
    }
}

#[tokio::test]
async fn disabling_lan_access_ends_its_tunnels() {
    let (h, _shutdown) = lan_host(false).await;
    let port = enable(&h).await;
    let (device, host_id) = paired_device(&h).await;
    let c = tunnel(&device, &host_id, loopback(port)).await;
    let mut stream = events(&c).await;

    h.ok("lan.set", json!({"enabled": false})).await;
    assert_eq!(closed(&mut stream).await, (None, None));
    assert!(matches!(c.request("host.hello", json!({})).await, Err(TunnelError::Closed)));
    // Local clients are unaffected.
    h.ok("host.hello", json!({})).await;

    let port = enable(&h).await;
    let c = tunnel(&device, &host_id, loopback(port)).await;
    ok(&h, &c, "host.hello", json!({})).await;
}

#[tokio::test]
async fn connections_beyond_the_limit_are_closed_on_arrival() {
    let (h, _shutdown) = lan_host(false).await;
    let addr = loopback(enable(&h).await);
    let mut held = Vec::new();
    for _ in 0..super::MAX_CONNECTIONS {
        held.push(dial(addr).await);
    }
    // Give the host time to accept every held connection.
    tokio::time::sleep(Duration::from_millis(200)).await;
    closed_silently(dial(addr).await).await;

    drop(held);
    let (device, host_id) = paired_device(&h).await;
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        match device.connect_tunnel(dial(addr).await, &host_id).await {
            Ok(t) => {
                ok(&h, &TunnelClient::start(t), "host.hello", json!({})).await;
                break;
            }
            Err(_) if tokio::time::Instant::now() < deadline => tokio::time::sleep(Duration::from_millis(50)).await,
            Err(e) => panic!("{e:?}"),
        }
    }
}

#[test]
fn the_advertisement_is_the_host_id_port_and_key_only() {
    let keys = HostKeys::generate();
    let host_id = uuid::Uuid::new_v4().to_string();
    let info = service_info(&host_id, &keys.static_pubkey(), 4711).unwrap();
    assert_eq!(info.get_fullname(), format!("{host_id}.{SERVICE_TYPE}"));
    assert_eq!(info.get_hostname(), format!("{host_id}.local."));
    assert_eq!(info.get_port(), 4711);
    let mut txt: Vec<(String, String)> = info
        .get_properties()
        .iter()
        .map(|p| (p.key().to_string(), p.val_str().to_string()))
        .collect();
    txt.sort();
    assert_eq!(
        txt,
        [("id".into(), host_id.clone()), ("pk".into(), keys.static_pubkey()), ("v".into(), "1".into())]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_host_is_found_over_mdns_and_reached_at_the_advertised_address() {
    let (h, _shutdown) = lan_host(true).await;
    let port = enable(&h).await;
    let (device, host_id) = paired_device(&h).await;
    let status = h.ok("lan.status", json!({})).await;
    assert_eq!(status["advertised"], true, "{status}");

    let browser = mdns_sd::ServiceDaemon::new().unwrap();
    let events = browser.browse(SERVICE_TYPE).unwrap();
    let want = format!("{host_id}.{SERVICE_TYPE}");
    let found = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            match events.recv_async().await.unwrap() {
                mdns_sd::ServiceEvent::ServiceResolved(s) if s.get_fullname() == want => break s,
                _ => {}
            }
        }
    })
    .await
    .expect("resolved over mDNS within 15s");
    let _ = browser.shutdown();

    assert_eq!(found.get_port(), port);
    let mut txt: Vec<(String, String)> =
        found.txt_properties.iter().map(|p| (p.key().to_string(), p.val_str().to_string())).collect();
    txt.sort();
    let pk = h.state.host_keys.get().unwrap().static_pubkey();
    assert_eq!(txt, [("id".into(), host_id.clone()), ("pk".into(), pk), ("v".into(), "1".into())]);
    let ip = found.get_addresses_v4().into_iter().next().expect("an IPv4 address");
    let c = tunnel(&device, &host_id, SocketAddr::from((ip, port))).await;
    assert_eq!(ok(&h, &c, "host.hello", json!({})).await["host_id"], host_id.as_str());

    // Turning LAN access off withdraws the advertisement.
    h.ok("lan.set", json!({"enabled": false})).await;
    assert_eq!(h.ok("lan.status", json!({})).await["advertised"], false);
}
