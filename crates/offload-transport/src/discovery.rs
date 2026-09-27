//! Finding peers on a LAN, without being told where they are.
//!
//! ADR-0015 keeps addresses inside this crate, and this is where they come from when nobody
//! has written a seed list: mDNS. A node advertises `_offload._udp.local.` with its node id
//! in the instance name, and browses for others doing the same.
//!
//! ## What is advertised, and what deliberately is not
//!
//! **The node id, and nothing about the fleet.** It is tempting to include a fleet tag so
//! that two fleets sharing a LAN do not dial each other — but *any* deterministic function of
//! the fleet public key is a verifier for an offline passphrase search (guess, derive, hash,
//! compare), and ADR-0012's whole defence is that the fleet key is only obtainable by holding
//! a certificate. Multicasting a fleet identifier would hand that to anyone on the coffee-shop
//! wifi, and turn mDNS into a fleet-membership oracle for a network full of strangers.
//!
//! The cost of leaving it out is a wasted dial: a node from another fleet is discovered,
//! dialled once, and refused at the handshake, which is a check that has to happen anyway. A
//! node id is already public — it is what a peer dials — so advertising it gives away nothing
//! that connecting does not.
//!
//! Discovery *proposes*; it never admits. What comes out of here is "somebody at this address
//! claims to be this key", and it is worth exactly as much as the handshake that follows.

use crate::TransportError;
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use offload_core::NodeId;
use std::net::SocketAddr;

/// The service type. `_udp` because the transport is QUIC, and the port advertised is the one
/// QUIC listens on.
const SERVICE: &str = "_offload._udp.local.";

/// A peer, as far as multicast can tell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovered {
    pub node: NodeId,
    /// Every address the peer advertised, best first. A laptop on wifi and a dock announces
    /// both, and only one of them reaches it — so the caller tries them in order rather than
    /// betting on the first.
    pub addresses: Vec<SocketAddr>,
}

/// This node, announced on the LAN for as long as this value is alive.
pub struct Advertisement {
    daemon: ServiceDaemon,
    fullname: String,
}

impl std::fmt::Debug for Advertisement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Advertisement")
            .field("service", &self.fullname)
            .finish()
    }
}

impl Drop for Advertisement {
    fn drop(&mut self) {
        // Withdraw rather than time out: a node that vanishes from the LAN and leaves its
        // advertisement behind sends every peer on a pointless dial for the next few minutes.
        let _ = self.daemon.unregister(&self.fullname);
        let _ = self.daemon.shutdown();
    }
}

/// Announce this node on the LAN, at the address it is actually reachable on.
///
/// **What is advertised has to be what was bound**, and getting that wrong is invisible from the
/// node doing it. A daemon bound to `127.0.0.1:7431` with the addresses filled in automatically
/// announces `192.168.x.x:7431`; a peer dials that, reaches nothing, and reports "no route" about
/// a machine that is running perfectly well two feet away. Twenty minutes, every time.
///
/// So: a wildcard bind advertises every interface and keeps them up to date, which is the case
/// that matters on a laptop moving between wifi and a dock; a specific bind advertises exactly
/// that address and nothing else. A loopback bind is not advertisable at all and is refused by
/// [`Advertisable::of`] before it gets here.
pub fn advertise(
    node: NodeId,
    bound: std::net::SocketAddr,
    name: &str,
) -> Result<Advertisement, TransportError> {
    let port = bound.port();
    let daemon = ServiceDaemon::new().map_err(|e| TransportError::io("starting mDNS", &e))?;

    // The full node id is a TXT property, not the instance name: a DNS label is limited to
    // 63 bytes and 32 bytes of hex is 64. The instance name gets the short form, which is
    // only there to be unique on the wire — the key that gets dialled comes from `node`.
    let full = node.to_string();
    let instance = format!("offload-{}", node.short());
    let host = format!("{instance}.local.");
    let properties = [("node", full.as_str()), ("name", name)];

    let wildcard = bound.ip().is_unspecified();
    // An empty address plus `enable_addr_auto` means "every interface, kept current". A specific
    // one means "this, and only this" — the whole point of passing the bound address in.
    let addresses = if wildcard {
        String::new()
    } else {
        bound.ip().to_string()
    };
    let service = ServiceInfo::new(
        SERVICE,
        &instance,
        &host,
        addresses.as_str(),
        port,
        &properties[..],
    )
    .map_err(|e| TransportError::io("describing this node for mDNS", &e))?;
    let service = if wildcard {
        service.enable_addr_auto()
    } else {
        service
    };
    let fullname = service.get_fullname().to_string();

    daemon
        .register(service)
        .map_err(|e| TransportError::io("advertising this node", &e))?;

    Ok(Advertisement { daemon, fullname })
}

/// The addresses a peer could dial this node on (ADR-0076): mDNS's rule, for gossip. A specific
/// bind is exactly that address. A wildcard bind is every interface's address with the bound
/// port, leaving out loopback and IPv6 link-local, which means nothing off this link without a
/// scope a peer cannot know. A loopback bind is nothing at all, as for [`Advertisable`].
#[must_use]
pub fn dialable_addresses(bound: std::net::SocketAddr) -> Vec<std::net::SocketAddr> {
    if bound.ip().is_loopback() {
        return Vec::new();
    }
    if !bound.ip().is_unspecified() {
        return vec![bound];
    }
    let v4_only = bound.is_ipv4();
    let mut out: Vec<std::net::SocketAddr> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        // A container or VM bridge is this machine's own private network: no other machine
        // can dial it, and the laptop stated six Docker bridges beside its two real addresses.
        .filter(|iface| !is_virtual_bridge(&iface.name))
        .map(|iface| iface.ip())
        .filter(|ip| !ip.is_loopback())
        .filter(|ip| match ip {
            std::net::IpAddr::V6(v6) => !v4_only && (v6.segments()[0] & 0xffc0) != 0xfe80,
            std::net::IpAddr::V4(_) => true,
        })
        .map(|ip| std::net::SocketAddr::new(ip, bound.port()))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// An interface for containers or virtual machines on this host, by the names Docker, Podman,
/// libvirt and the kernel give them.
fn is_virtual_bridge(name: &str) -> bool {
    [
        "docker", "br-", "veth", "virbr", "podman", "cni", "flannel", "lxc", "lxd",
    ]
    .iter()
    .any(|prefix| name.starts_with(prefix))
}

/// Whether a bound address is one a peer could dial, and why not when it is not.
///
/// Its own type rather than a bool for the usual reason here: the caller has to *say* why it is
/// not announcing, and "mDNS is off" and "this node bound loopback" are two different situations
/// with two different fixes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Advertisable {
    /// Announce it.
    Yes,
    /// Nothing on the LAN can reach a loopback address, and nothing loopback-bound can reach the
    /// LAN either — so this is not a degraded announcement, it is no announcement at all.
    Loopback,
}

impl Advertisable {
    #[must_use]
    pub fn of(bound: std::net::SocketAddr) -> Advertisable {
        if bound.ip().is_loopback() {
            Advertisable::Loopback
        } else {
            Advertisable::Yes
        }
    }
}

/// Watch for other nodes.
pub struct Browser {
    daemon: ServiceDaemon,
    events: mdns_sd::Receiver<ServiceEvent>,
    /// Ourselves, so we do not report our own advertisement as a discovery.
    local: NodeId,
}

impl std::fmt::Debug for Browser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Browser")
            .field("local", &self.local.short())
            .finish()
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        let _ = self.daemon.shutdown();
    }
}

/// Start browsing. Peers arrive from [`Browser::next`] as they answer.
pub fn browse(local: NodeId) -> Result<Browser, TransportError> {
    let daemon = ServiceDaemon::new().map_err(|e| TransportError::io("starting mDNS", &e))?;
    let events = daemon
        .browse(SERVICE)
        .map_err(|e| TransportError::io("browsing for peers", &e))?;
    Ok(Browser {
        daemon,
        events,
        local,
    })
}

impl Browser {
    /// The next peer worth trying, or `None` when the browser has stopped.
    ///
    /// Everything that is not a resolved peer is skipped rather than surfaced: mDNS is chatty,
    /// and the caller's job is to dial, not to filter multicast.
    pub async fn next(&self) -> Option<Discovered> {
        loop {
            let event = self.events.recv_async().await.ok()?;
            let ServiceEvent::ServiceResolved(service) = event else {
                continue;
            };
            let Some(node) = node_of(&service.txt_properties) else {
                continue;
            };
            if node == self.local {
                continue;
            }
            tracing::debug!(node = %node.short(), service = %service.fullname, "resolved a peer");

            let addresses = dialable(&service);
            if !addresses.is_empty() {
                return Some(Discovered { node, addresses });
            }
        }
    }
}

/// The advertised addresses worth trying, in the order to try them.
///
/// IPv4 first, because it is the one most likely to work without help. IPv6 link-local
/// addresses are dropped rather than sorted last: reaching one needs the scope of the
/// interface it was heard on, and an address that cannot be dialled is a timeout wearing the
/// costume of a peer.
fn dialable(service: &mdns_sd::ResolvedService) -> Vec<SocketAddr> {
    let mut addresses: Vec<SocketAddr> = service
        .addresses
        .iter()
        .map(|ip| SocketAddr::new(ip.to_ip_addr(), service.port))
        .filter(|address| match address.ip() {
            std::net::IpAddr::V4(_) => true,
            std::net::IpAddr::V6(ip) => !ip.is_unicast_link_local(),
        })
        .collect();
    addresses.sort_by_key(|address| (address.is_ipv6(), address.to_string()));
    addresses.truncate(4);
    addresses
}

/// The node id an advertisement claims.
///
/// A claim, not a fact — the handshake is what settles it. Anything unparseable is somebody
/// else's service on the same multicast group, or a version of this that has moved on.
fn node_of(properties: &mdns_sd::TxtProperties) -> Option<NodeId> {
    properties.get_property_val_str("node")?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(b: u8) -> NodeId {
        NodeId::from_bytes([b; 32])
    }

    #[test]
    fn an_advertisement_names_the_node_and_says_nothing_about_the_fleet() {
        // The property this module is careful about: any deterministic function of the fleet
        // key is a verifier for an offline passphrase search, so none of it goes on the wire
        // (ADR-0012). A node id is already public — it is what a peer dials.
        let node = id(7);
        let instance = format!("offload-{}", node.short());
        let info = ServiceInfo::new(
            SERVICE,
            &instance,
            &format!("{instance}.local."),
            "",
            7433,
            &[("node", node.to_string().as_str()), ("name", "laptop")][..],
        )
        .expect("describe");

        let text = format!("{:?}", info.get_properties());
        assert!(text.contains("laptop"));
        assert!(!text.to_lowercase().contains("fleet"));
        assert_eq!(node_of(info.get_properties()), Some(node));

        // A DNS label is 63 bytes and a node id is 64 characters of hex, so the id cannot be
        // the label. Getting this wrong publishes an advertisement nobody can resolve.
        for label in info.get_fullname().split('.') {
            assert!(label.len() <= 63, "{label} is not a legal DNS label");
        }
    }

    #[test]
    fn a_loopback_node_is_not_advertised_and_a_real_one_is() {
        use std::net::SocketAddr;

        // The twenty-minute bug: `enable_addr_auto` publishes every interface, so a daemon bound
        // to loopback announced its LAN address, and a peer dialled a port nothing was listening
        // on. Refused before the announcement is built, rather than announced wrongly.
        assert_eq!(
            Advertisable::of("127.0.0.1:7431".parse::<SocketAddr>().expect("addr")),
            Advertisable::Loopback
        );
        assert_eq!(
            Advertisable::of("[::1]:7431".parse::<SocketAddr>().expect("addr")),
            Advertisable::Loopback
        );
        // A wildcard bind really is on every interface, so filling them in is correct there.
        assert_eq!(
            Advertisable::of("0.0.0.0:7431".parse::<SocketAddr>().expect("addr")),
            Advertisable::Yes
        );
        assert_eq!(
            Advertisable::of("192.168.1.20:7431".parse::<SocketAddr>().expect("addr")),
            Advertisable::Yes
        );
    }

    #[test]
    fn an_advertisement_from_something_else_entirely_is_ignored() {
        // Multicast is a shared bus: everything on it that is not one of ours has to fall out
        // here rather than becoming a dial to a printer.
        let info = ServiceInfo::new(
            SERVICE,
            "some-printer",
            "some-printer.local.",
            "",
            9100,
            &[("what", "not ours")][..],
        )
        .expect("describe");
        assert_eq!(node_of(info.get_properties()), None);
    }
}

#[cfg(test)]
mod dialable_tests {
    use super::*;

    /// ADR-0076: what is gossiped follows mDNS's rule — the bound address when specific, every
    /// interface when wildcard, and never loopback or IPv6 link-local.
    #[test]
    fn what_is_gossiped_is_what_a_peer_could_dial() {
        let specific: std::net::SocketAddr = "192.0.2.5:7601".parse().expect("addr");
        assert_eq!(dialable_addresses(specific), vec![specific]);
        let loopback: std::net::SocketAddr = "127.0.0.1:7601".parse().expect("addr");
        assert!(dialable_addresses(loopback).is_empty());
        assert!(is_virtual_bridge("docker0") && is_virtual_bridge("br-55cc7ee2e231"));
        assert!(
            !is_virtual_bridge("enp58s0u1")
                && !is_virtual_bridge("wlp3s0")
                && !is_virtual_bridge("en0")
        );
        let wildcard: std::net::SocketAddr = "[::]:7601".parse().expect("addr");
        for a in dialable_addresses(wildcard) {
            assert_eq!(a.port(), 7601);
            assert!(!a.ip().is_loopback(), "{a}");
            if let std::net::IpAddr::V6(v6) = a.ip() {
                assert_ne!(v6.segments()[0] & 0xffc0, 0xfe80, "{a}");
            }
        }
    }
}
