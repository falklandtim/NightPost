//! Keeping fetches the sender controls off your own network.
//!
//! Some addresses NightPost fetches come from an email: its pictures (the
//! MCP's `view_message`) and the sender's domain (its logo, `avatars.rs` in
//! the app). An email could point them at a machine on your network, such
//! as a router's admin page, a printer or a Tailscale peer. Fetches whose host is,
//! or resolves to, a private, loopback, link-local or shared (CGNAT,
//! Tailscale) address are refused, as are single-word hosts (`router`).
//!
//! The check sits in the HTTP client's own name lookup ([`agent`]), so the
//! address checked is the address connected to: a name can't be checked
//! once and then resolve somewhere else for the download.

use std::net::{IpAddr, SocketAddr, ToSocketAddrs};

/// Is `ip` on the public internet (not this machine or a local network)?
pub fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            !(v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_multicast()
                // 100.64.0.0/10: carrier-grade NAT, and Tailscale's addresses.
                || (a == 100 && (64..128).contains(&b))
                || a == 0)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public_ip(IpAddr::V4(v4));
            }
            let first = v6.segments()[0];
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                // fc00::/7 unique local; fe80::/10 link-local.
                || (first & 0xfe00) == 0xfc00
                || (first & 0xffc0) == 0xfe80)
        }
    }
}

/// Could `host` be on the public internet, judging by the name alone (no
/// lookup)? A public address, or a dotted name that isn't `localhost`.
pub fn plausible_public_name(host: &str) -> bool {
    let h = host.trim().trim_start_matches('[').trim_end_matches(']').trim_end_matches('.');
    if let Ok(ip) = h.parse::<IpAddr>() {
        return is_public_ip(ip);
    }
    h.contains('.') && !h.eq_ignore_ascii_case("localhost") && !h.to_ascii_lowercase().ends_with(".localhost")
}

/// May NightPost fetch from `host`, which resolves to `addresses`? Only a
/// [`plausible_public_name`] all of whose addresses are public.
pub fn host_allowed(host: &str, addresses: &[IpAddr]) -> bool {
    let h = host.trim().trim_start_matches('[').trim_end_matches(']');
    if h.parse::<IpAddr>().is_ok() {
        return plausible_public_name(h);
    }
    plausible_public_name(h) && !addresses.is_empty() && addresses.iter().all(|ip| is_public_ip(*ip))
}

/// Name lookup for [`agent`]: refuses hosts [`host_allowed`] says no to.
struct PublicOnly;

impl ureq::Resolver for PublicOnly {
    fn resolve(&self, netloc: &str) -> std::io::Result<Vec<SocketAddr>> {
        let addrs: Vec<SocketAddr> = netloc.to_socket_addrs()?.collect();
        let host = netloc.rsplit_once(':').map_or(netloc, |(h, _)| h);
        let ips: Vec<IpAddr> = addrs.iter().map(SocketAddr::ip).collect();
        if host_allowed(host, &ips) {
            Ok(addrs)
        } else {
            Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "that address is on a private network"))
        }
    }
}

/// An HTTP client that only connects to public addresses, for fetches an
/// email decides (pictures, sender logos).
pub fn agent(builder: ureq::AgentBuilder) -> ureq::Agent {
    builder.resolver(PublicOnly).build()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ips(list: &[&str]) -> Vec<IpAddr> {
        list.iter().map(|s| s.parse().unwrap()).collect()
    }

    #[test]
    fn private_and_local_hosts_are_refused() {
        for host in ["127.0.0.1", "10.0.0.5", "192.168.1.1", "172.16.0.9", "169.254.1.1", "100.99.223.90", "[::1]", "::1", "fe80::1", "fd00::5", "::ffff:192.168.1.1", "0.0.0.0", "router", "localhost", "printer.localhost"] {
            assert!(!host_allowed(host, &ips(&["93.184.215.14"])), "{host}");
        }
        // A public name that points at a private address is refused too.
        assert!(!host_allowed("evil.example", &ips(&["192.168.1.1"])));
        assert!(!host_allowed("evil.example", &ips(&["93.184.215.14", "10.0.0.1"])));
        assert!(!host_allowed("nowhere.example", &[]));
        assert!(host_allowed("example.com", &ips(&["93.184.215.14", "2606:2800:21f:cb07:6820:80da:af6b:8b2c"])));
        assert!(host_allowed("93.184.215.14", &[]));
        assert!(host_allowed("100.128.0.1", &[]), "just past the shared range");
    }
}
