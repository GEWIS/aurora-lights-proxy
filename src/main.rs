use anyhow::{bail, Context, Result};
use rust_socketio::client::Client;
use rust_socketio::{ClientBuilder, Event, Payload};
use serde_json::json;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tracing::{debug, error, info, warn};

use aurora_lights_proxy::artnet::ArtNetSender;
use aurora_lights_proxy::config::Config;
use aurora_lights_proxy::packet::parse_array;

// If a namespace stays disconnected this long, tear down `run()` and let the
// outer loop re-authenticate. rust_socketio reconnects on its own with a
// 1-60s backoff, so 45s gives it plenty of attempts before we escalate.
const DEAD_THRESHOLD: Duration = Duration::from_secs(45);

// Reset the outer-loop backoff counter after this much continuous uptime.
const STABLE_UPTIME: Duration = Duration::from_secs(60);

// rust_socketio's built-in reconnect bounds (milliseconds).
const RECONNECT_DELAY_MIN_MS: u64 = 1_000;
const RECONNECT_DELAY_MAX_MS: u64 = 60_000;

fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    let config = Config::from_env()?;
    init_logging(&config.log_level);

    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = Arc::clone(&stop);
        ctrlc::set_handler(move || {
            info!("Interrupt received, shutting down...");
            stop.store(true, Ordering::SeqCst);
        })
        .context("failed to register signal handler")?;
    }

    let mut consecutive_failures: u32 = 0;
    while !stop.load(Ordering::SeqCst) {
        let run_started = Instant::now();
        match run(&config, &stop) {
            Ok(()) => break,
            Err(e) => {
                error!("Run ended: {e:#}");
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                if run_started.elapsed() >= STABLE_UPTIME {
                    consecutive_failures = 0;
                }
                let delay = backoff(consecutive_failures);
                consecutive_failures = consecutive_failures.saturating_add(1);
                info!(
                    "Retrying in {}s (attempt {consecutive_failures})...",
                    delay.as_secs()
                );
                sleep_interruptible(delay, &stop);
            }
        }
    }

    info!("Goodbye");
    Ok(())
}

/// Exponential backoff capped at 60s: 1, 2, 4, 8, 16, 32, 60, 60, ...
fn backoff(attempt: u32) -> Duration {
    let shift = attempt.min(6);
    let secs = (1u64 << shift).min(60);
    Duration::from_secs(secs)
}

/// Sleep in 100ms chunks so Ctrl+C is responsive.
fn sleep_interruptible(total: Duration, stop: &Arc<AtomicBool>) {
    let mut remaining = total;
    while remaining > Duration::ZERO {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        let chunk = remaining.min(Duration::from_millis(100));
        thread::sleep(chunk);
        remaining = remaining.saturating_sub(chunk);
    }
}

/// Per-namespace connection state. The flag tracks the latest event, the
/// timestamp records when the flag last flipped -- together they let the
/// watchdog tell "transient hiccup" from "stuck disconnected for too long".
struct ConnState {
    connected: AtomicBool,
    last_change: Mutex<Instant>,
}

impl ConnState {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            connected: AtomicBool::new(false),
            last_change: Mutex::new(Instant::now()),
        })
    }

    fn mark_connected(&self) {
        self.connected.store(true, Ordering::SeqCst);
        *self
            .last_change
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Instant::now();
    }

    fn mark_disconnected(&self) {
        self.connected.store(false, Ordering::SeqCst);
        *self
            .last_change
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Instant::now();
    }

    fn is_dead(&self, threshold: Duration) -> bool {
        if self.connected.load(Ordering::SeqCst) {
            return false;
        }
        self.last_change
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .elapsed()
            > threshold
    }
}

#[allow(clippy::too_many_lines)]
fn run(config: &Config, stop: &Arc<AtomicBool>) -> Result<()> {
    let cookie = authenticate(config)?;
    let cookie_header = format!("connect.sid={cookie}");

    let artnet = ArtNetSender::new(
        config.target_ip,
        config.universe,
        config.packet_size,
        config.fps,
    )?;
    let packet_size = config.packet_size as usize;

    let lights_state = ConnState::new();
    let lights_artnet = artnet.clone();
    let lights_on_connect = {
        let s = Arc::clone(&lights_state);
        let art = artnet.clone();
        move |_: Payload, _: rust_socketio::RawClient| {
            info!("Connected to /lights");
            s.mark_connected();
            // Re-blackout + restart on every (re)connect so the sender thread
            // is alive and the buffer is in a known state.
            art.blackout();
            art.start();
        }
    };
    let lights_on_close = {
        let s = Arc::clone(&lights_state);
        move |_: Payload, _: rust_socketio::RawClient| {
            warn!("Lost /lights connection, rust_socketio will retry");
            s.mark_disconnected();
        }
    };
    let lights_client = ClientBuilder::new(&config.url)
        .namespace("/lights")
        .opening_header("Cookie", cookie_header.clone())
        .reconnect(true)
        .reconnect_on_disconnect(true)
        .reconnect_delay(RECONNECT_DELAY_MIN_MS, RECONNECT_DELAY_MAX_MS)
        .on("dmx_packet", move |payload, _| {
            handle_dmx(&lights_artnet, payload, packet_size);
        })
        .on(Event::Connect, lights_on_connect)
        .on(Event::Close, lights_on_close)
        .on(Event::Error, |err, _| {
            error!("/lights socket error: {err:?}");
        })
        .connect()
        .context("failed to connect to /lights namespace")?;

    let main_state = ConnState::new();
    let main_on_connect = {
        let s = Arc::clone(&main_state);
        move |_: Payload, _: rust_socketio::RawClient| {
            info!("Connected to Aurora core");
            s.mark_connected();
        }
    };
    let main_on_close = {
        let s = Arc::clone(&main_state);
        let art = artnet.clone();
        move |_: Payload, _: rust_socketio::RawClient| {
            warn!("Lost connection to Aurora core, rust_socketio will retry");
            s.mark_disconnected();
            // Black out so the fixtures don't get stuck on whatever the last
            // frame was during the outage.
            art.blackout();
        }
    };
    let main_client = ClientBuilder::new(&config.url)
        .namespace("/")
        .opening_header("Cookie", cookie_header)
        .reconnect(true)
        .reconnect_on_disconnect(true)
        .reconnect_delay(RECONNECT_DELAY_MIN_MS, RECONNECT_DELAY_MAX_MS)
        .on(Event::Connect, main_on_connect)
        .on(Event::Close, main_on_close)
        .on(Event::Error, |err, _| error!("core socket error: {err:?}"))
        .connect()
        .context("failed to connect to default namespace")?;

    let main_client = Arc::new(main_client);

    let latency_ms = Arc::new(AtomicU64::new(0));
    let start_time = Instant::now();

    let status_handle = {
        let main_client = Arc::clone(&main_client);
        let latency_ms = Arc::clone(&latency_ms);
        let stop = Arc::clone(stop);
        thread::spawn(move || status_loop(&main_client, &latency_ms, start_time, &stop))
    };

    // Health watchdog: tear down `run()` (and trigger the outer loop's re-auth)
    // if either namespace stays disconnected past DEAD_THRESHOLD.
    let result: Result<()> = loop {
        if stop.load(Ordering::SeqCst) {
            break Ok(());
        }
        if lights_state.is_dead(DEAD_THRESHOLD) {
            break Err(anyhow::anyhow!(
                "/lights stayed disconnected for >{DEAD_THRESHOLD:?}, re-authenticating"
            ));
        }
        if main_state.is_dead(DEAD_THRESHOLD) {
            break Err(anyhow::anyhow!(
                "core stayed disconnected for >{DEAD_THRESHOLD:?}, re-authenticating"
            ));
        }
        thread::sleep(Duration::from_millis(500));
    };

    info!("Tearing down clients");
    artnet.stop();
    artnet.blackout();
    let _ = main_client.disconnect();
    let _ = lights_client.disconnect();
    let _ = status_handle.join();

    result
}

fn handle_dmx(artnet: &ArtNetSender, payload: Payload, packet_size: usize) {
    match payload {
        Payload::Binary(bytes) => {
            let take = bytes.len().min(packet_size);
            artnet.set(&bytes[..take]);
        }
        Payload::Text(values) => {
            let arr = values.iter().find_map(|v| v.as_array());
            let Some(arr) = arr else {
                warn!("dmx_packet text payload did not contain an array");
                return;
            };
            // Clamp at i64 before narrowing: an out-of-range wire value like
            // 2_147_483_648 would otherwise wrap to a negative i32 and end up
            // as 0 instead of saturating at 255.
            let ints: Vec<i32> = arr
                .iter()
                .map(|v| v.as_i64().unwrap_or(0).clamp(0, 255) as i32)
                .collect();
            let buf = parse_array(&ints, packet_size);
            artnet.set(&buf);
        }
        #[allow(deprecated)]
        Payload::String(s) => match serde_json::from_str::<Vec<i32>>(&s) {
            Ok(arr) => {
                let buf = parse_array(&arr, packet_size);
                artnet.set(&buf);
            }
            Err(e) => warn!("dmx_packet string payload parse failed: {e}"),
        },
    }
}

fn status_loop(
    client: &Arc<Client>,
    latency_ms: &Arc<AtomicU64>,
    start_time: Instant,
    stop: &Arc<AtomicBool>,
) {
    while !stop.load(Ordering::SeqCst) {
        let uptime_seconds = start_time.elapsed().as_secs();
        let system_timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(0));

        let payload = json!({
            "uptimeSeconds": uptime_seconds,
            "systemTimestamp": system_timestamp,
            "latencyMilliseconds": latency_ms.load(Ordering::Relaxed),
        });

        let send_time = Instant::now();
        let latency_for_cb = Arc::clone(latency_ms);
        let emit_result = client.emit_with_ack(
            "status:update",
            payload,
            Duration::from_secs(2),
            move |_, _| {
                let rtt = send_time.elapsed();
                let half = u64::try_from(rtt.as_millis()).unwrap_or(0) / 2;
                latency_for_cb.store(half, Ordering::Relaxed);
                debug!("Latency: {half} ms");
            },
        );
        if let Err(e) = emit_result {
            // Don't crash the loop -- the watchdog handles dead connections.
            warn!("status:update emit failed: {e:?}");
        }

        sleep_interruptible(Duration::from_secs(5), stop);
    }
}

fn authenticate(config: &Config) -> Result<String> {
    let url = format!("{}/api/auth/key", config.url.trim_end_matches('/'));
    let client = reqwest::blocking::Client::builder()
        .cookie_store(true)
        .timeout(Duration::from_secs(10))
        .build()
        .context("failed to build HTTP client")?;

    let mut params = std::collections::HashMap::new();
    params.insert("key", config.api_key.as_str());

    let resp = client
        .post(&url)
        .form(&params)
        .send()
        .with_context(|| format!("auth request to {url} failed"))?;

    let status = resp.status();
    if !status.is_success() {
        let body: serde_json::Value = resp.json().unwrap_or_else(|_| json!({}));
        let detail = body
            .get("details")
            .and_then(|v| v.as_str())
            .or_else(|| body.get("message").and_then(|v| v.as_str()))
            .unwrap_or("unknown error");
        bail!("Could not authenticate with core: [HTTP {status}]: {detail}");
    }

    for cookie in resp.cookies() {
        if cookie.name() == "connect.sid" {
            return Ok(cookie.value().to_string());
        }
    }
    bail!("Authentication response did not contain a connect.sid cookie")
}

fn init_logging(level: &str) {
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(level))
        .unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = fmt().with_env_filter(filter).with_target(false).try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_then_caps() {
        assert_eq!(backoff(0), Duration::from_secs(1));
        assert_eq!(backoff(1), Duration::from_secs(2));
        assert_eq!(backoff(2), Duration::from_secs(4));
        assert_eq!(backoff(3), Duration::from_secs(8));
        assert_eq!(backoff(4), Duration::from_secs(16));
        assert_eq!(backoff(5), Duration::from_secs(32));
        assert_eq!(backoff(6), Duration::from_secs(60));
        // Caps from here on out
        assert_eq!(backoff(7), Duration::from_secs(60));
        assert_eq!(backoff(100), Duration::from_secs(60));
        assert_eq!(backoff(u32::MAX), Duration::from_secs(60));
    }

    #[test]
    fn conn_state_starts_disconnected_but_fresh() {
        let s = ConnState::new();
        // Just-constructed state: not connected yet but timestamp is recent,
        // so the watchdog gives it a grace window.
        assert!(!s.is_dead(Duration::from_secs(1)));
    }

    #[test]
    fn conn_state_dies_after_threshold() {
        let s = ConnState::new();
        s.mark_disconnected();
        // Backdate last_change so the elapsed time exceeds the threshold.
        *s.last_change.lock().unwrap() = Instant::now() - Duration::from_secs(10);
        assert!(s.is_dead(Duration::from_secs(5)));
        assert!(!s.is_dead(Duration::from_secs(60)));
    }

    #[test]
    fn conn_state_alive_when_connected() {
        let s = ConnState::new();
        s.mark_disconnected();
        *s.last_change.lock().unwrap() = Instant::now() - Duration::from_secs(120);
        // Reconnect -- watchdog should immediately consider the link alive.
        s.mark_connected();
        assert!(!s.is_dead(Duration::from_secs(5)));
    }
}
