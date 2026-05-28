use anyhow::{bail, Context, Result};
use std::env;
use std::fmt;
use std::net::Ipv4Addr;

/// Art-Net addresses universes with 15 bits (8-bit subuni + 7-bit net).
pub const MAX_UNIVERSE: u16 = 32_767;
pub const MIN_PACKET_SIZE: u16 = 2;
pub const MAX_PACKET_SIZE: u16 = 512;

#[derive(Clone)]
pub struct Config {
    pub url: String,
    pub api_key: String,
    pub log_level: String,
    pub target_ip: Ipv4Addr,
    pub universe: u16,
    pub packet_size: u16,
    pub fps: u32,
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Custom impl so api_key never lands in logs or error reports.
        f.debug_struct("Config")
            .field("url", &self.url)
            .field("api_key", &"<redacted>")
            .field("log_level", &self.log_level)
            .field("target_ip", &self.target_ip)
            .field("universe", &self.universe)
            .field("packet_size", &self.packet_size)
            .field("fps", &self.fps)
            .finish()
    }
}

impl Config {
    /// # Errors
    ///
    /// Returns an error if any required env var is absent or if any value
    /// fails validation (universe range, packet size, FPS).
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            url: env::var("URL").context("URL env var is required")?,
            api_key: env::var("API_KEY").context("API_KEY env var is required")?,
            log_level: env::var("LOG_LEVEL").unwrap_or_else(|_| "info".to_string()),
            target_ip: parse_env_or("TARGET_IP", Ipv4Addr::new(169, 254, 0, 2))?,
            universe: validate_universe(parse_env_or("UNIVERSE", 0u16)?)?,
            packet_size: validate_packet_size(parse_env_or("PACKET_SIZE", 512u16)?)?,
            fps: validate_fps(parse_env_or("FPS", 40u32)?)?,
        })
    }
}

fn validate_universe(universe: u16) -> Result<u16> {
    if universe > MAX_UNIVERSE {
        bail!("UNIVERSE must be in 0..={MAX_UNIVERSE}, got {universe}");
    }
    Ok(universe)
}

fn validate_packet_size(packet_size: u16) -> Result<u16> {
    if !(MIN_PACKET_SIZE..=MAX_PACKET_SIZE).contains(&packet_size) {
        bail!("PACKET_SIZE must be in {MIN_PACKET_SIZE}..={MAX_PACKET_SIZE}, got {packet_size}");
    }
    if !packet_size.is_multiple_of(2) {
        bail!("PACKET_SIZE must be even, got {packet_size}");
    }
    Ok(packet_size)
}

fn validate_fps(fps: u32) -> Result<u32> {
    if fps == 0 {
        bail!("FPS must be greater than 0");
    }
    Ok(fps)
}

fn parse_env_or<T>(key: &str, default: T) -> Result<T>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    match env::var(key) {
        Ok(v) => v
            .parse::<T>()
            .map_err(|e| anyhow::anyhow!("invalid value for {key}: {e}")),
        Err(_) => Ok(default),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_env_requires_url_and_key() {
        // Run in isolation: ensure required keys are absent.
        // We use temp_env-like manual handling.
        let prev_url = env::var("URL").ok();
        let prev_key = env::var("API_KEY").ok();
        env::remove_var("URL");
        env::remove_var("API_KEY");

        let err = Config::from_env().unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("URL"), "expected URL error, got: {msg}");

        if let Some(v) = prev_url {
            env::set_var("URL", v);
        }
        if let Some(v) = prev_key {
            env::set_var("API_KEY", v);
        }
    }

    #[test]
    fn parse_env_or_returns_default_when_unset() {
        env::remove_var("NONEXISTENT_TEST_KEY_XYZ");
        let v: u16 = parse_env_or("NONEXISTENT_TEST_KEY_XYZ", 42u16).unwrap();
        assert_eq!(v, 42);
    }

    #[test]
    fn parse_env_or_uses_env_value() {
        env::set_var("TEST_PARSE_KEY", "99");
        let v: u16 = parse_env_or("TEST_PARSE_KEY", 1u16).unwrap();
        assert_eq!(v, 99);
        env::remove_var("TEST_PARSE_KEY");
    }

    fn sample_config() -> Config {
        Config {
            url: "http://localhost:3000".into(),
            api_key: "super-secret-do-not-log".into(),
            log_level: "info".into(),
            target_ip: Ipv4Addr::new(169, 254, 0, 2),
            universe: 0,
            packet_size: 512,
            fps: 40,
        }
    }

    #[test]
    fn debug_redacts_api_key() {
        let cfg = sample_config();
        let rendered = format!("{cfg:?}");
        assert!(
            !rendered.contains("super-secret-do-not-log"),
            "api_key leaked in Debug output: {rendered}"
        );
        assert!(
            rendered.contains("<redacted>"),
            "Debug output should mark the api_key as redacted: {rendered}"
        );
    }

    #[test]
    fn validate_universe_accepts_zero() {
        assert_eq!(validate_universe(0).unwrap(), 0);
    }

    #[test]
    fn validate_universe_accepts_max() {
        assert_eq!(validate_universe(MAX_UNIVERSE).unwrap(), MAX_UNIVERSE);
    }

    #[test]
    fn validate_universe_rejects_over_max() {
        let err = validate_universe(MAX_UNIVERSE + 1).unwrap_err();
        assert!(format!("{err:#}").contains("UNIVERSE"));
    }

    #[test]
    fn validate_packet_size_rejects_below_min() {
        let err = validate_packet_size(0).unwrap_err();
        assert!(format!("{err:#}").contains("PACKET_SIZE"));
    }

    #[test]
    fn validate_packet_size_rejects_odd() {
        let err = validate_packet_size(511).unwrap_err();
        assert!(format!("{err:#}").contains("even"));
    }

    #[test]
    fn validate_packet_size_rejects_oversized() {
        let err = validate_packet_size(514).unwrap_err();
        assert!(format!("{err:#}").contains("PACKET_SIZE"));
    }

    #[test]
    fn validate_packet_size_accepts_boundaries() {
        assert_eq!(
            validate_packet_size(MIN_PACKET_SIZE).unwrap(),
            MIN_PACKET_SIZE
        );
        assert_eq!(
            validate_packet_size(MAX_PACKET_SIZE).unwrap(),
            MAX_PACKET_SIZE
        );
    }

    #[test]
    fn validate_fps_rejects_zero() {
        assert!(validate_fps(0).is_err());
        assert_eq!(validate_fps(1).unwrap(), 1);
    }
}
