//! SSH X11 forwarding (`x11-req`) onto a local display.
//!
//! The server opens an `x11` channel when a remote client connects to the
//! fake display. The first bytes are an X11 setup that still carries the
//! fake cookie from `x11-req`. That cookie is checked and replaced with the
//! local display's cookie before any byte is written to the real server.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use russh::{Channel, client};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Clone)]
struct Bridge {
    target: crate::platform::x11_server::DisplayTarget,
    fake: [u8; 16],
    real: [u8; 16],
}

fn bridge_for_display(
    current: &mut Option<Bridge>,
    target: &crate::platform::x11_server::DisplayTarget,
    cookie: [u8; 16],
) -> Bridge {
    if current.as_ref().is_none_or(|bridge| bridge.target != *target || bridge.real != cookie) {
        *current = Some(Bridge { target: target.clone(), fake: random_cookie(), real: cookie });
    }
    current.clone().expect("bridge was just installed")
}

fn slots() -> &'static Mutex<HashMap<String, Arc<Mutex<Option<Bridge>>>>> {
    static SLOTS: OnceLock<Mutex<HashMap<String, Arc<Mutex<Option<Bridge>>>>>> = OnceLock::new();
    SLOTS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn slot(key: &str) -> Arc<Mutex<Option<Bridge>>> {
    let mut slots = slots().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    slots.entry(key.to_owned()).or_insert_with(|| Arc::new(Mutex::new(None))).clone()
}

pub(super) fn accept_channel(pool_key: &str, channel: Channel<client::Msg>) {
    let Some(bridge) =
        slot(pool_key).lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    else {
        log::warn!("SSH X11 channel ignored because this connection has no display");
        return;
    };
    let runtime = match super::runtime() {
        Ok(runtime) => runtime,
        Err(error) => {
            log::warn!("SSH X11 channel ignored: {error}");
            return;
        },
    };
    runtime.spawn(async move {
        if let Err(error) = proxy(channel, &bridge).await {
            log::warn!("SSH X11 channel closed: {error}");
        }
    });
}

pub(super) async fn request(
    pool_key: &str,
    channel: &mut super::lifecycle::ShellChannel,
) -> Result<crate::platform::x11_server::DisplayLease, String> {
    let display = crate::platform::x11_server::ensure()?;
    let bridge = {
        let cell = slot(pool_key);
        let mut current = cell.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        bridge_for_display(&mut current, &display.target, display.cookie)
    };
    send_request(channel, &bridge).await?;
    Ok(display.lease)
}

pub(super) async fn request_again(
    pool_key: &str,
    channel: &mut super::lifecycle::ShellChannel,
) -> Result<(), String> {
    let bridge = slot(pool_key)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
        .ok_or_else(|| "X11 display is no longer active".to_owned())?;
    send_request(channel, &bridge).await
}

async fn send_request(
    channel: &mut super::lifecycle::ShellChannel,
    bridge: &Bridge,
) -> Result<(), String> {
    channel
        .request_x11(false, "MIT-MAGIC-COOKIE-1", &hex_encode(&bridge.fake), 0)
        .await
        .map_err(|error| format!("the remote server rejected X11 forwarding: {error}"))
}

async fn proxy(channel: Channel<client::Msg>, bridge: &Bridge) -> Result<(), String> {
    let mut remote = channel.into_stream();
    let mut header = [0u8; 12];
    remote.read_exact(&mut header).await.map_err(|error| error.to_string())?;
    let setup_len = setup_length(&header).map_err(|_| "X11 setup is truncated".to_owned())?;
    let mut setup = vec![0u8; setup_len];
    setup[..12].copy_from_slice(&header);
    if setup_len > 12 {
        remote.read_exact(&mut setup[12..]).await.map_err(|error| error.to_string())?;
    }
    let setup = rewrite_client_setup(&setup, &bridge.fake, &bridge.real)
        .map_err(|_| "X11 authentication cookie was rejected".to_owned())?;
    match &bridge.target {
        crate::platform::x11_server::DisplayTarget::Tcp { host, port } => {
            let mut local = tokio::net::TcpStream::connect((host.as_str(), *port))
                .await
                .map_err(|error| format!("local X display is unavailable: {error}"))?;
            local.write_all(&setup).await.map_err(|error| error.to_string())?;
            let _ = tokio::io::copy_bidirectional(&mut remote, &mut local).await;
        },
        #[cfg(unix)]
        crate::platform::x11_server::DisplayTarget::Unix(path) => {
            let mut local = tokio::net::UnixStream::connect(path)
                .await
                .map_err(|error| format!("local X display is unavailable: {error}"))?;
            local.write_all(&setup).await.map_err(|error| error.to_string())?;
            let _ = tokio::io::copy_bidirectional(&mut remote, &mut local).await;
        },
        #[cfg(not(unix))]
        crate::platform::x11_server::DisplayTarget::Unix(_) => {
            return Err("Unix socket X11 display is unsupported on this platform".to_owned());
        },
    }
    Ok(())
}

fn setup_length(header: &[u8]) -> Result<usize, ()> {
    if header.len() < 12 {
        return Err(());
    }
    let name_len = read_u16(header, 6)?;
    let data_len = read_u16(header, 8)?;
    let name_pad = (4 - (name_len % 4)) % 4;
    let data_pad = (4 - (data_len % 4)) % 4;
    Ok(12 + name_len + name_pad + data_len + data_pad)
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<usize, ()> {
    let pair = bytes.get(offset..offset + 2).ok_or(())?;
    let value = match bytes.first().copied() {
        Some(b'B') => u16::from_be_bytes([pair[0], pair[1]]),
        Some(b'l') => u16::from_le_bytes([pair[0], pair[1]]),
        _ => return Err(()),
    };
    Ok(usize::from(value))
}

fn rewrite_client_setup(bytes: &[u8], fake: &[u8; 16], real: &[u8; 16]) -> Result<Vec<u8>, ()> {
    let setup_len = setup_length(bytes)?;
    if bytes.len() < setup_len {
        return Err(());
    }
    let name_len = read_u16(bytes, 6)?;
    let data_len = read_u16(bytes, 8)?;
    let name = bytes.get(12..12 + name_len).ok_or(())?;
    if name != b"MIT-MAGIC-COOKIE-1" || data_len != 16 {
        return Err(());
    }
    let name_pad = (4 - (name_len % 4)) % 4;
    let data_start = 12 + name_len + name_pad;
    let data = bytes.get(data_start..data_start + 16).ok_or(())?;
    if data != fake {
        return Err(());
    }
    let mut rewritten = bytes.to_vec();
    rewritten[data_start..data_start + 16].copy_from_slice(real);
    Ok(rewritten)
}

fn random_cookie() -> [u8; 16] {
    let mut cookie = [0u8; 16];
    getrandom::fill(&mut cookie).expect("X11 cookie entropy");
    cookie
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restarting_the_local_display_replaces_the_pooled_bridge_cookie() {
        use crate::platform::x11_server::DisplayTarget;
        let target = DisplayTarget::Tcp { host: "127.0.0.1".into(), port: 6020 };
        let mut current = None;
        let first = bridge_for_display(&mut current, &target, [1; 16]);
        let shared = bridge_for_display(&mut current, &target, [1; 16]);
        assert_eq!(first.fake, shared.fake);
        let restarted = bridge_for_display(&mut current, &target, [2; 16]);
        assert_eq!(restarted.real, [2; 16]);
        assert_ne!(restarted.fake, first.fake);
        let moved = DisplayTarget::Tcp { host: "127.0.0.1".into(), port: 6021 };
        assert_eq!(bridge_for_display(&mut current, &moved, [2; 16]).target, moved);
    }

    fn setup(order: u8, cookie: &[u8; 16]) -> Vec<u8> {
        let name = b"MIT-MAGIC-COOKIE-1";
        let mut bytes = vec![order, 0, 0, 11, 0, 0, 0, name.len() as u8, 0, 16, 0, 0];
        if order == b'l' {
            bytes[6] = name.len() as u8;
            bytes[7] = 0;
            bytes[8] = 16;
            bytes[9] = 0;
        }
        bytes.extend_from_slice(name);
        bytes.extend_from_slice(&[0, 0]);
        bytes.extend_from_slice(cookie);
        bytes
    }

    #[test]
    fn a_matching_cookie_is_replaced_and_a_wrong_one_is_rejected() {
        let fake = [1u8; 16];
        let real = [2u8; 16];
        let rewritten = rewrite_client_setup(&setup(b'B', &fake), &fake, &real).unwrap();
        assert_eq!(&rewritten[rewritten.len() - 16..], &real);
        let rewritten_le = rewrite_client_setup(&setup(b'l', &fake), &fake, &real).unwrap();
        assert_eq!(&rewritten_le[rewritten_le.len() - 16..], &real);
        assert!(rewrite_client_setup(&setup(b'l', &fake), &[3u8; 16], &real).is_err());
        assert!(rewrite_client_setup(&setup(b'B', &fake)[..8], &fake, &real).is_err());

        let mut wrong_proto = setup(b'B', &fake);
        wrong_proto[12..15].copy_from_slice(b"BAD");
        assert!(rewrite_client_setup(&wrong_proto, &fake, &real).is_err());
    }
}
