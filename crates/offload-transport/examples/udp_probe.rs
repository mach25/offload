//! Send a datagram the way quinn sends one, with nothing else in the way.
//!
//! Built for one question: a Mac and a Linux laptop on one switch carry every datagram python
//! can throw at them — 1200 and 1400 bytes, bursts of ten, the daemons' exact 4-tuple, and
//! ECT0-marked — while `offloadd`'s QUIC handshake between the same two hosts gets no reply and
//! the sender's own `sendmsg` once returned `EHOSTUNREACH`. Writing to a quinn stream succeeds
//! locally whether or not the bytes leave, so nothing above this layer can tell the difference.
//!
//! This goes under quinn to `quinn-udp`: the same `sendmsg` path, the same ECN marking, no TLS,
//! no certificate, no connection state. If these datagrams arrive and QUIC's do not, the fault
//! is above the syscall; if they vanish where python's arrive, it is at or below it.
//!
//! ```text
//! # on the receiver (any host)
//! python3 -c "import socket;s=socket.socket(2,2);s.bind(('0.0.0.0',9500));print(s.recvfrom(4000))"
//! # on the sender
//! cargo run --release -p offload-transport --example udp_probe -- 192.0.2.51 192.0.2.240:9500 10
//! ```
// A diagnostic binary, not a production path: it is run by hand with arguments and its whole
// job is to fail loudly and immediately when one of them is wrong. Same exemption an integration
// test takes, and stated here because `examples/` does not get it for free.
#![allow(clippy::expect_used)]

use std::net::{SocketAddr, UdpSocket};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!(
            "usage: udp_probe <local-ip> <peer-ip:port> [count] [size] [interval-ms] \
             [local-port] [ect0|none|alternate]"
        );
        std::process::exit(2);
    }
    let local: std::net::IpAddr = args[1].parse().expect("local ip");
    let peer: SocketAddr = args[2].parse().expect("peer addr");
    let count: usize = args.get(3).map_or(5, |c| c.parse().expect("count"));
    let size: usize = args.get(4).map_or(1200, |s| s.parse().expect("size"));
    // **The two arguments that make this a different question.** With an interval the probe
    // holds *one* socket across the whole run instead of being a fresh socket per invocation,
    // and with a local port it holds the same one the daemon binds. Both matter: a Mac daemon
    // whose `sendmsg` had been failing to a peer for minutes was measured against 122 datagrams
    // from this probe to the same address, in the same window, with **zero** failures — and the
    // only things not held constant were the socket's age and its port.
    let interval = args
        .get(5)
        .map_or(0u64, |i| i.parse().expect("interval-ms"));
    let port: u16 = args.get(6).map_or(0, |p| p.parse().expect("local-port"));
    // **The variable nothing had held constant.** quinn marks every datagram ECT0, and it does
    // it as a `sendmsg` *control message*; every "raw UDP works" control in `docs/DEMO.md` was
    // either python with no cmsg at all or python with `IP_TOS` as a socket *option*, which is
    // a different call into the kernel. `alternate` sends odd datagrams marked and even ones
    // bare, on one socket, milliseconds apart, so the two arms cannot drift apart in time.
    let ecn_arg = args.get(7).map_or("ect0", |e| e.as_str());

    // Bound to the same address the daemon binds, because a socket bound to one interface of a
    // multi-homed host is what produced `EHOSTUNREACH` here in the first place.
    let socket = UdpSocket::bind(SocketAddr::new(local, port)).expect("bind");
    println!("bound {}", socket.local_addr().expect("local addr"));

    let state = quinn_udp::UdpSocketState::new((&socket).into()).expect("udp state");
    println!(
        "quinn-udp: gso={} gro={}",
        state.max_gso_segments(),
        state.gro_segments()
    );

    let payload = vec![b'z'; size];
    let mut marked = (0usize, 0usize);
    let mut bare = (0usize, 0usize);
    let mut sendto_after_failure = (0usize, 0usize);
    for n in 0..count {
        if interval > 0 && n > 0 {
            std::thread::sleep(std::time::Duration::from_millis(interval));
        }
        let ecn = match ecn_arg {
            "none" => None,
            "alternate" if n % 2 == 1 => None,
            _ => Some(quinn_udp::EcnCodepoint::Ect0),
        };
        let transmit = quinn_udp::Transmit {
            destination: peer,
            ecn,
            contents: &payload,
            segment_size: None,
            src_ip: None,
        };
        // `UdpSocketState::send` swallows everything but `WouldBlock` — it logs at most one
        // line a minute and returns `Ok`, because UDP errors are the caller's problem to
        // retransmit around. That is exactly why a daemon can fail every send for minutes and
        // report nothing, so this probe uses `try_send`, which hands the error back.
        let tally = if ecn.is_some() {
            &mut marked
        } else {
            &mut bare
        };
        match state.try_send((&socket).into(), &transmit) {
            Ok(()) => tally.0 += 1,
            Err(e) => {
                tally.1 += 1;
                // **The tightest control there is: the same fd, microseconds later.**
                // `UdpSocket::send_to` is `sendto`; `quinn-udp` is `sendmsg` with a control
                // message. Everything else — socket, options, route, ARP, destination, size,
                // the instant — is held constant by construction, because it is one socket in
                // one process. If this succeeds where the line above failed, the difference is
                // the syscall and its message header, and nothing outside the process.
                match socket.send_to(&payload, peer) {
                    Ok(_) => sendto_after_failure.0 += 1,
                    Err(e2) => {
                        sendto_after_failure.1 += 1;
                        if sendto_after_failure.1 <= 2 {
                            println!("datagram {n}: sendto also failed: {e2}");
                        }
                    }
                }
                if tally.1 <= 3 {
                    println!("datagram {n}: sendmsg failed: {e} (kind {:?})", e.kind());
                }
            }
        }
    }
    println!("{size}-byte datagrams to {peer}, one socket:");
    println!("  ECT0-marked: {} ok, {} failed", marked.0, marked.1);
    println!("  unmarked:    {} ok, {} failed", bare.0, bare.1);
    if sendto_after_failure.0 + sendto_after_failure.1 > 0 {
        println!(
            "  `sendto` on the same fd, straight after each failure: {} ok, {} failed",
            sendto_after_failure.0, sendto_after_failure.1
        );
    }
}
