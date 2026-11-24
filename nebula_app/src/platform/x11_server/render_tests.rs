//! Exercise image transfer against the real, explicitly supplied Windows X server.
use super::*;

#[test]
#[ignore = "launches VcXsrv; set PEBREL_VCXSRV_TEST_EXE"]
fn real_vcxsrv_authenticates_and_round_trips_image_pixels() {
    let executable = std::env::var_os("PEBREL_VCXSRV_TEST_EXE").expect("explicit test runtime");
    let display = start_server(Path::new(&executable)).unwrap();
    let DisplayTarget::Tcp { port, .. } = display.target else { panic!("TCP display") };
    struct Owner(Option<Running>);
    impl Drop for Owner {
        fn drop(&mut self) {
            if let Some(running) = self.0.take() {
                stop(running);
            }
        }
    }
    let authority = display.running.authority.clone().unwrap();
    let owner = Owner(Some(display.running));
    probe_display(port, &display.cookie).unwrap();
    assert!(probe_display(port, &[0; 16]).is_err());
    roundtrip_pixels(port, &display.cookie);
    drop(owner);
    assert!(!authority.exists());
    assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_err());
}

fn roundtrip_pixels(port: u16, cookie: &[u8; 16]) {
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    let timeout = Some(std::time::Duration::from_secs(2));
    stream.set_read_timeout(timeout).unwrap();
    stream.set_write_timeout(timeout).unwrap();
    stream.write_all(&[b'l', 0, 11, 0, 0, 0, 18, 0, 16, 0, 0, 0]).unwrap();
    stream.write_all(b"MIT-MAGIC-COOKIE-1\0\0").unwrap();
    stream.write_all(cookie).unwrap();
    let mut header = [0; 8];
    stream.read_exact(&mut header).unwrap();
    assert_eq!(header[0], 1, "authenticated setup");
    let mut setup = vec![0; usize::from(u16::from_le_bytes([header[6], header[7]])) * 4];
    stream.read_exact(&mut setup).unwrap();
    let u32_at = |offset| u32::from_le_bytes(setup[offset..offset + 4].try_into().unwrap());
    let base = u32_at(4);
    assert_eq!(setup[22], 0, "little-endian image data");
    assert!(setup[20] > 0, "at least one screen");
    let vendor_len = usize::from(u16::from_le_bytes([setup[16], setup[17]]));
    let formats_start = 32 + (vendor_len + 3) / 4 * 4;
    let screen = formats_start + usize::from(setup[21]) * 8;
    let root = u32_at(screen);
    let depth = setup[screen + 38];
    assert!(setup[formats_start..screen].chunks_exact(8).any(|f| f[0] == depth && f[1] == 32));
    let pixmap = base | 1;
    let gc = base | 2;
    let mut request = vec![53, depth, 4, 0]; // CreatePixmap
    request.extend_from_slice(&pixmap.to_le_bytes());
    request.extend_from_slice(&root.to_le_bytes());
    request.extend_from_slice(&[2, 0, 2, 0]);
    stream.write_all(&request).unwrap();
    let mut request = vec![55, 0, 4, 0]; // CreateGC, default graphics state
    request.extend_from_slice(&gc.to_le_bytes());
    request.extend_from_slice(&pixmap.to_le_bytes());
    request.extend_from_slice(&0u32.to_le_bytes());
    stream.write_all(&request).unwrap();
    let colors = [0x00336699u32, 0x00ee2200, 0x0011cc44, 0x00abcdef];
    let mut request = vec![72, 2, 10, 0]; // PutImage, ZPixmap, 2 x 2
    request.extend_from_slice(&pixmap.to_le_bytes());
    request.extend_from_slice(&gc.to_le_bytes());
    request.extend_from_slice(&[2, 0, 2, 0, 0, 0, 0, 0, 0, depth, 0, 0]);
    for color in colors {
        request.extend_from_slice(&color.to_le_bytes());
    }
    stream.write_all(&request).unwrap();
    let mut request = vec![73, 2, 5, 0]; // GetImage
    request.extend_from_slice(&pixmap.to_le_bytes());
    request.extend_from_slice(&[0, 0, 0, 0, 2, 0, 2, 0]);
    request.extend_from_slice(&u32::MAX.to_le_bytes());
    stream.write_all(&request).unwrap();
    let mut reply = [0; 32];
    stream.read_exact(&mut reply).unwrap();
    assert_eq!(reply[0], 1, "image request failed: {reply:?}");
    let length = u32::from_le_bytes(reply[4..8].try_into().unwrap()) as usize * 4;
    assert_eq!(length, 16);
    let mut pixels = [0; 16];
    stream.read_exact(&mut pixels).unwrap();
    for (actual, expected) in pixels.chunks_exact(4).zip(colors) {
        assert_eq!(u32::from_le_bytes(actual.try_into().unwrap()) & 0x00ffffff, expected);
    }
}
