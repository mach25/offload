//! Does the receive path ever coalesce, and does this project's testing exercise it?
//!
//! `udp_probe.rs` answered the *send* question and cleared everything below `quinn-proto`. It
//! also printed, and nobody wrote down, `gro=64`: `quinn-udp` opportunistically turns on
//! `UDP_GRO`, so a Linux receiver can be handed **one buffer holding up to 64 datagrams** and
//! must split it itself at `stride` boundaries. That split is userspace, which puts it *above*
//! the kernel counters `/proc/net/snmp` was read for and *below* where `quinn_proto=trace`
//! logs — the one gap in the Linux↔macOS record where a datagram can vanish with both
//! instruments reading clean.
//!
//! The reason to suspect it is untested rather than merely unexamined: every multi-node test in
//! this project's history was daemons on **one Linux box**, and same-host traffic never crosses
//! a NIC. So this asks a question that needs no second machine: on the paths available here,
//! does GRO coalesce at all?
//!
//! - `stride == len` on every message: no coalescing, so the split loop never ran, so no test
//!   here has ever exercised it.
//! - `len > stride`: coalescing happens, the split is being exercised, and the bytes are
//!   checked below to say whether it is correct.
//!
//! ```text
//! cargo run --release -p offload-transport --example gro_probe            # loopback
//! cargo run --release -p offload-transport --example gro_probe -- 192.0.2.51 16 1200
//! ```
// A diagnostic binary run by hand, whose whole job is to fail loudly on a bad argument. Same
// exemption `udp_probe.rs` takes, and stated here because `examples/` does not get it for free.
#![allow(clippy::expect_used)]

use std::io::IoSliceMut;
use std::net::{IpAddr, SocketAddr, UdpSocket};

fn main() {
    let args: Vec<String> = std::env::args().collect();

    // `listen <bind-addr> [expected-count]` receives only, so the sender can be another host —
    // which is the whole point, since a same-host path never crosses a NIC.
    let listen = args.get(1).map(String::as_str) == Some("listen");
    let (receiver, sender, count, size) = if listen {
        let bind: SocketAddr = args
            .get(2)
            .expect("usage: gro_probe listen <bind-ip:port> [expected-count] [size]")
            .parse()
            .expect("bind addr");
        let count: usize = args.get(3).map_or(16, |c| c.parse().expect("count"));
        let size: usize = args.get(4).map_or(1200, |s| s.parse().expect("size"));
        (
            UdpSocket::bind(bind).expect("bind receiver"),
            None,
            count,
            size,
        )
    } else {
        let local: IpAddr = args
            .get(1)
            .map_or("127.0.0.1", String::as_str)
            .parse()
            .expect("local ip");
        let count: usize = args.get(2).map_or(16, |c| c.parse().expect("count"));
        let size: usize = args.get(3).map_or(1200, |s| s.parse().expect("size"));
        let receiver = UdpSocket::bind(SocketAddr::new(local, 0)).expect("bind receiver");
        let sender = UdpSocket::bind(SocketAddr::new(local, 0)).expect("bind sender");
        (receiver, Some(sender), count, size)
    };
    let to = receiver.local_addr().expect("receiver addr");

    // Exactly how the daemon's socket is set up: this is what turns UDP_GRO on.
    let state = quinn_udp::UdpSocketState::new((&receiver).into()).expect("udp state");
    println!(
        "receiver {to}  gso={} gro={}",
        state.max_gso_segments(),
        state.gro_segments()
    );

    // Each datagram is filled with its own index, so a mis-split is visible as a wrong byte
    // rather than only as a wrong count.
    match &sender {
        Some(sender) => {
            for i in 0..count {
                let payload = vec![u8::try_from(i % 251).expect("fits"); size];
                sender.send_to(&payload, to).expect("send");
            }
            println!("sent {count} datagrams of {size} bytes");
        }
        None => println!("waiting for {count} datagrams of {size} bytes from another host"),
    }

    // Sized the way quinn sizes it: room for a full GRO list, so a short read cannot be
    // mistaken for a lost datagram.
    let mut buf = vec![0u8; size * state.gro_segments().max(1) * 2];
    let mut metas = [quinn_udp::RecvMeta::default(); 1];

    // `UdpSocketState::new` put the socket in **non-blocking** mode — that is the first thing it
    // does, and it silently makes `set_read_timeout` inert. An earlier version of this probe read
    // once, got `WouldBlock` before the sender had even started, and reported zero datagrams over
    // a path that was carrying every one of them. So the wait is explicit here, and a zero can
    // only mean the deadline passed with nothing arriving.
    let deadline = std::time::Instant::now()
        + std::time::Duration::from_millis(if listen { 15_000 } else { 500 });

    let (mut datagrams, mut recvs, mut coalesced, mut bad) = (0usize, 0usize, 0usize, 0usize);
    loop {
        if std::time::Instant::now() >= deadline {
            break;
        }
        let mut bufs = [IoSliceMut::new(&mut buf)];
        match state.recv((&receiver).into(), &mut bufs, &mut metas) {
            Ok(msgs) => {
                for meta in metas.iter().take(msgs) {
                    recvs += 1;
                    let here = meta.len.div_ceil(meta.stride.max(1));
                    datagrams += here;
                    if meta.len > meta.stride {
                        coalesced += 1;
                        println!(
                            "  recv: len={} stride={} -> {here} datagrams (coalesced)",
                            meta.len, meta.stride
                        );
                    }
                    // Every byte of a segment must equal that segment's index. A split at the
                    // wrong offset shows up here even when the count happens to come out right.
                    let mut offset = 0usize;
                    while offset < meta.len {
                        let end = (offset + meta.stride).min(meta.len);
                        let seg = &bufs[0][offset..end];
                        if let Some(&first) = seg.first() {
                            if !seg.iter().all(|&b| b == first) {
                                bad += 1;
                                println!("  MIS-SPLIT: segment at {offset} is not uniform");
                            }
                        }
                        offset = end;
                    }
                }
            }
            // Nothing ready yet. On a non-blocking socket this is the ordinary case, not the end
            // of the traffic, so it waits for the deadline rather than concluding anything.
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                if datagrams >= count {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            Err(e) => {
                println!("recv error: {e}");
                break;
            }
        }
    }

    println!("\n{recvs} recv calls returned {datagrams} of {count} datagrams");
    println!("{coalesced} of those calls carried a coalesced buffer");
    if bad > 0 {
        println!("{bad} segments did not survive the split");
    }
    if coalesced == 0 {
        println!(
            "VERDICT: no coalescing on this path, so quinn's split loop never ran here.\n\
             A test on this path cannot have exercised it."
        );
    } else if bad == 0 && datagrams == count {
        println!("VERDICT: coalescing happened and every datagram survived it.");
    } else {
        println!("VERDICT: coalescing happened and datagrams were lost or mis-split.");
    }
}
