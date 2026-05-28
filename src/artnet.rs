use anyhow::{Context, Result};
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use tracing::{debug, warn};

use crate::packet::{build_artnet_frame, ARTNET_PORT};

#[derive(Clone)]
pub struct ArtNetSender {
    inner: Arc<Inner>,
}

struct Inner {
    socket: UdpSocket,
    target: SocketAddrV4,
    universe: u16,
    packet_size: u16,
    fps: u32,
    data: Mutex<Vec<u8>>,
    running: AtomicBool,
    handle: Mutex<Option<JoinHandle<()>>>,
}

impl ArtNetSender {
    /// # Errors
    ///
    /// Returns an error if the UDP socket cannot be bound or configured for broadcast.
    pub fn new(target_ip: Ipv4Addr, universe: u16, packet_size: u16, fps: u32) -> Result<Self> {
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
            .context("failed to bind UDP socket for Art-Net")?;
        socket
            .set_broadcast(true)
            .context("failed to enable broadcast on Art-Net socket")?;

        let target = SocketAddrV4::new(target_ip, ARTNET_PORT);
        let data = vec![0u8; packet_size as usize];

        Ok(Self {
            inner: Arc::new(Inner {
                socket,
                target,
                universe,
                packet_size,
                fps,
                data: Mutex::new(data),
                running: AtomicBool::new(false),
                handle: Mutex::new(None),
            }),
        })
    }

    /// Replace the current DMX payload. Excess bytes are dropped and short
    /// payloads are padded with zeros.
    pub fn set(&self, data: &[u8]) {
        let mut buf = lock_recover(&self.inner.data);
        let n = data.len().min(buf.len());
        buf[..n].copy_from_slice(&data[..n]);
        for byte in buf.iter_mut().skip(n) {
            *byte = 0;
        }
    }

    /// Set every channel to zero.
    pub fn blackout(&self) {
        let mut buf = lock_recover(&self.inner.data);
        for byte in buf.iter_mut() {
            *byte = 0;
        }
        // Best-effort: blast one immediate frame so the fixtures go dark even
        // if the send loop is not running.
        let frame = build_artnet_frame(self.inner.universe, 0, &buf);
        if let Err(e) = self.inner.socket.send_to(&frame, self.inner.target) {
            warn!("blackout send failed: {e}");
        }
    }

    /// Start the background thread that re-sends the current frame at the
    /// configured FPS. Calling `start` while already running is a no-op.
    pub fn start(&self) {
        if self.inner.running.swap(true, Ordering::SeqCst) {
            return;
        }

        let inner = Arc::clone(&self.inner);
        let interval = Duration::from_micros(1_000_000 / u64::from(inner.fps.max(1)));

        let handle = thread::spawn(move || {
            debug!(
                "Art-Net loop started: target={}, universe={}, fps={}",
                inner.target, inner.universe, inner.fps
            );

            while inner.running.load(Ordering::SeqCst) {
                let snapshot = lock_recover(&inner.data).clone();
                let frame = build_artnet_frame(inner.universe, 0, &snapshot);
                if let Err(e) = inner.socket.send_to(&frame, inner.target) {
                    // UDP send errors typically mean the local interface is
                    // down (ENETUNREACH / EHOSTUNREACH). We can't tell from
                    // here whether the controller itself failed -- when it
                    // comes back, frames will flow again automatically.
                    warn!("Art-Net send failed: {e}");
                }
                thread::sleep(interval);
            }

            debug!("Art-Net loop stopped");
        });

        *lock_recover(&self.inner.handle) = Some(handle);
    }

    /// Stop the background thread and block until it exits.
    pub fn stop(&self) {
        if !self.inner.running.swap(false, Ordering::SeqCst) {
            return;
        }
        if let Some(handle) = lock_recover(&self.inner.handle).take() {
            let _ = handle.join();
        }
    }

    #[must_use]
    pub fn packet_size(&self) -> u16 {
        self.inner.packet_size
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
    }
}

// Recover the inner value if the mutex was poisoned by a panicking thread.
// We never want a single panic in a callback to wedge the proxy.
fn lock_recover<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::UdpSocket;
    use std::time::Duration;

    fn fresh_sender(packet_size: u16) -> (ArtNetSender, UdpSocket) {
        // Bind a receiver on loopback to a free port and point the sender at
        // it. We construct the sender first, then rewrite its target via
        // returning a fresh sender bound to that ephemeral port.
        let receiver = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        receiver
            .set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();
        let port = receiver.local_addr().unwrap().port();

        let sender_socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        sender_socket.set_broadcast(true).unwrap();
        let inner = Inner {
            socket: sender_socket,
            target: SocketAddrV4::new(Ipv4Addr::LOCALHOST, port),
            universe: 0,
            packet_size,
            fps: 200,
            data: Mutex::new(vec![0u8; packet_size as usize]),
            running: AtomicBool::new(false),
            handle: Mutex::new(None),
        };
        (
            ArtNetSender {
                inner: Arc::new(inner),
            },
            receiver,
        )
    }

    #[test]
    fn set_truncates_oversized_payload() {
        let (sender, _r) = fresh_sender(8);
        sender.set(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        let buf = sender.inner.data.lock().unwrap();
        assert_eq!(*buf, vec![1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn set_zero_pads_short_payload() {
        let (sender, _r) = fresh_sender(8);
        sender.set(&[9, 9, 9]);
        let buf = sender.inner.data.lock().unwrap();
        assert_eq!(*buf, vec![9, 9, 9, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn start_sends_frames_over_udp() {
        let (sender, receiver) = fresh_sender(4);
        sender.set(&[1, 2, 3, 4]);
        sender.start();

        let mut buf = [0u8; 1024];
        let (n, _) = receiver
            .recv_from(&mut buf)
            .expect("no Art-Net frame received");
        sender.stop();

        assert_eq!(n, 18 + 4);
        assert_eq!(&buf[0..8], b"Art-Net\0");
        assert_eq!(&buf[18..22], &[1, 2, 3, 4]);
    }

    #[test]
    fn blackout_zeroes_and_sends_immediate_frame() {
        let (sender, receiver) = fresh_sender(4);
        sender.set(&[1, 2, 3, 4]);
        sender.blackout();

        let buf = sender.inner.data.lock().unwrap();
        assert!(buf.iter().all(|&b| b == 0));
        drop(buf);

        let mut rxbuf = [0u8; 1024];
        let (n, _) = receiver
            .recv_from(&mut rxbuf)
            .expect("no blackout frame received");
        assert_eq!(&rxbuf[18..n], &[0, 0, 0, 0]);
    }
}
