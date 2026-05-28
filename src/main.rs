use anyhow::{bail, Context, Result};
use rust_socketio::client::Client;
use rust_socketio::{ClientBuilder, Event, Payload};
use serde_json::json;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tracing::{debug, error, info, warn};

use aurora_lights_proxy::artnet::ArtNetSender;
use aurora_lights_proxy::config::Config;
use aurora_lights_proxy::packet::parse_array;

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

    while !stop.load(Ordering::SeqCst) {
        match run(&config, &stop) {
            Ok(()) => break,
            Err(e) => {
                error!("Something went wrong: {e:#}");
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                info!("Retrying in 5 seconds...");
                for _ in 0..50 {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    thread::sleep(Duration::from_millis(100));
                }
            }
        }
    }

    info!("Goodbye");
    Ok(())
}

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

    let lights_artnet = artnet.clone();
    let lights_client = ClientBuilder::new(&config.url)
        .namespace("/lights")
        .opening_header("Cookie", cookie_header.clone())
        .on("dmx_packet", move |payload, _| {
            handle_dmx(&lights_artnet, payload, packet_size);
        })
        .on(Event::Error, |err, _| {
            error!("lights socket error: {err:?}")
        })
        .connect()
        .context("failed to connect to /lights namespace")?;

    let latency_ms = Arc::new(AtomicU64::new(0));
    let start_time = Instant::now();

    let connect_artnet = artnet.clone();
    let disconnect_artnet = artnet.clone();
    let main_client = ClientBuilder::new(&config.url)
        .namespace("/")
        .opening_header("Cookie", cookie_header)
        .on(Event::Connect, move |_, _| {
            info!("Connected to Aurora core");
            connect_artnet.blackout();
            connect_artnet.start();
        })
        .on(Event::Close, move |_, _| {
            info!("Disconnected from Aurora core");
            disconnect_artnet.stop();
            disconnect_artnet.blackout();
        })
        .on(Event::Error, |err, _| error!("core socket error: {err:?}"))
        .connect()
        .context("failed to connect to default namespace")?;

    let main_client = Arc::new(main_client);

    let status_handle = {
        let main_client = Arc::clone(&main_client);
        let latency_ms = Arc::clone(&latency_ms);
        let stop = Arc::clone(stop);
        thread::spawn(move || status_loop(main_client, latency_ms, start_time, stop))
    };

    while !stop.load(Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(250));
    }

    info!("Stopping proxy");
    artnet.stop();
    artnet.blackout();
    let _ = main_client.disconnect();
    let _ = lights_client.disconnect();
    let _ = status_handle.join();

    Ok(())
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
    client: Arc<Client>,
    latency_ms: Arc<AtomicU64>,
    start_time: Instant,
    stop: Arc<AtomicBool>,
) {
    while !stop.load(Ordering::SeqCst) {
        let uptime_seconds = start_time.elapsed().as_secs();
        let system_timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let payload = json!({
            "uptimeSeconds": uptime_seconds,
            "systemTimestamp": system_timestamp,
            "latencyMilliseconds": latency_ms.load(Ordering::Relaxed),
        });

        let send_time = Instant::now();
        let latency_for_cb = Arc::clone(&latency_ms);
        let emit_result = client.emit_with_ack(
            "status:update",
            payload,
            Duration::from_secs(2),
            move |_, _| {
                let rtt = send_time.elapsed();
                let half = (rtt.as_millis() as u64) / 2;
                latency_for_cb.store(half, Ordering::Relaxed);
                debug!("Latency: {half} ms");
            },
        );
        if let Err(e) = emit_result {
            warn!("status:update emit failed: {e:?}");
        }

        for _ in 0..50 {
            if stop.load(Ordering::SeqCst) {
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }
    }
}

fn authenticate(config: &Config) -> Result<String> {
    let url = format!("{}/api/auth/key", config.url.trim_end_matches('/'));
    let client = reqwest::blocking::Client::builder()
        .cookie_store(true)
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
