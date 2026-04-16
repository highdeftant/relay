//! Per-profile gateway connectivity checks.
//!
//! Maps Hermes profile names to their webhook listener endpoints and
//! provides a simple TCP-level health probe for each.

use std::collections::HashMap;
use std::net::TcpStream;
use std::time::Duration;

/// Describes a single profile's webhook endpoint.
#[derive(Debug, Clone)]
pub struct ProfileEndpoint {
    pub profile: String,
    pub host: String,
    pub port: u16,
}

/// Result of a connectivity probe for one profile.
#[derive(Debug, Clone)]
pub struct ProfileHealth {
    pub profile: String,
    pub endpoint: String,
    pub reachable: bool,
    pub latency_ms: Option<u64>,
}

/// Default profile-to-endport mapping matching current Hermes webhook config.
pub fn default_profile_endpoints() -> Vec<ProfileEndpoint> {
    vec![
        ProfileEndpoint {
            profile: "default".into(),
            host: "127.0.0.1".into(),
            port: 8644,
        },
        ProfileEndpoint {
            profile: "spoof".into(),
            host: "127.0.0.1".into(),
            port: 8645,
        },
        ProfileEndpoint {
            profile: "tracie".into(),
            host: "127.0.0.1".into(),
            port: 8646,
        },
    ]
}

/// Check reachability of a single endpoint via TCP connect.
fn probe_endpoint(ep: &ProfileEndpoint) -> ProfileHealth {
    let addr = format!("{}:{}", ep.host, ep.port);
    let start = std::time::Instant::now();
    let reachable = TcpStream::connect_timeout(
        &addr.parse().unwrap_or_else(|_| {
            std::net::SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), ep.port)
        }),
        Duration::from_secs(2),
    )
    .is_ok();
    let latency_ms = start.elapsed().as_millis() as u64;

    ProfileHealth {
        profile: ep.profile.clone(),
        endpoint: addr.clone(),
        reachable,
        latency_ms: if reachable { Some(latency_ms) } else { None },
    }
}

/// Probe all configured profile endpoints and return their health status.
pub fn check_all_endpoints(endpoints: &[ProfileEndpoint]) -> Vec<ProfileHealth> {
    endpoints.iter().map(probe_endpoint).collect()
}

/// Convenience: check defaults and return as a name->health map.
pub fn check_defaults() -> HashMap<String, ProfileHealth> {
    let results = check_all_endpoints(&default_profile_endpoints());
    results
        .into_iter()
        .map(|h| (h.profile.clone(), h))
        .collect()
}

/// Format a human-readable status line for one profile.
pub fn format_health_line(h: &ProfileHealth) -> String {
    if h.reachable {
        let ms = h.latency_ms.unwrap_or(0);
        format!("  ● {} @ {} ({}ms)", h.profile, h.endpoint, ms)
    } else {
        format!("  ○ {} @ {} (unreachable)", h.profile, h.endpoint)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_endpoints_has_three_profiles() {
        let eps = default_profile_endpoints();
        assert_eq!(eps.len(), 3);
        let names: Vec<&str> = eps.iter().map(|e| e.profile.as_str()).collect();
        assert!(names.contains(&"default"));
        assert!(names.contains(&"spoof"));
        assert!(names.contains(&"tracie"));
    }

    #[test]
    fn probe_unreachable_port_returns_false() {
        let ep = ProfileEndpoint {
            profile: "test".into(),
            host: "127.0.0.1".into(),
            port: 19999, // unlikely to be listening
        };
        let health = probe_endpoint(&ep);
        assert!(!health.reachable);
        assert!(health.latency_ms.is_none());
    }

    #[test]
    fn format_health_line_shows_status() {
        let h = ProfileHealth {
            profile: "spoof".into(),
            endpoint: "127.0.0.1:8645".into(),
            reachable: true,
            latency_ms: Some(3),
        };
        let line = format_health_line(&h);
        assert!(line.contains("●"));
        assert!(line.contains("spoof"));
        assert!(line.contains("3ms"));
    }
}
