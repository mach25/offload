//! What this machine would not send, counted where quinn throws it away.
//!
//! `quinn_udp::UdpSocketState::send` returns `Ok(())` for every send error but `WouldBlock`,
//! and its doc comment gives the reason: UDP transmission errors are non-fatal, because a
//! protocol built on UDP has to retransmit anyway. That is right, and nothing here disputes it
//! — a transport that tore a connection down because one `sendmsg` returned `EPERM` would be
//! worse than the silence.
//!
//! What is wrong is that the silence is *attributed to the peer*. A node whose kernel is
//! refusing every datagram believes it sent them all, so the failure detector reports
//! `no answer within 500ms` — **the peer did not reply** — for what is really **this machine
//! would not let me speak**. Session sixty-six spent three sessions on a macOS 26 laptop that
//! was mute for exactly this reason (`docs/DEMO.md`), and the only evidence that ever named
//! the real cause was one `EHOSTUNREACH` seen by hand under `strace`.
//!
//! So this counts rather than decides. [`WatchedSocket`] calls `try_send`, which hands the
//! error back, records it against the destination, and then returns exactly what quinn's own
//! socket would have returned. Behaviour is unchanged by construction; what is added is that
//! somebody can ask.

use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{ready, Context, Poll};

use quinn::udp::{RecvMeta, Transmit, UdpSocketState};
use quinn::{AsyncUdpSocket, UdpPoller};

/// How many destinations are remembered by name.
///
/// A fleet is a person's own devices, so this is generous — but the destinations here are
/// whatever was *dialled*, which includes a seed list and anything mDNS turned up, and a node
/// on a busy network can meet more of those than it has peers. Past the cap the count still
/// rises; only the breakdown stops growing, which is the half that could grow without bound.
const REMEMBERED: usize = 16;

/// What became of one datagram handed to the kernel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sent {
    /// The socket is full. quinn's contract is that this comes back, so it can wait for
    /// writability and try again — the one error that is not ours to swallow.
    Blocked,
    /// A probe bigger than the path allows, or a segmented batch the NIC cannot offload. quinn
    /// discovering what the path and the adapter will take, not a muzzle.
    Probing,
    /// The kernel would not send it. Discarded above, and the only one worth counting.
    Refused,
}

/// `segmented` is whether the transmit was a GSO batch (`Transmit::segment_size`).
fn classify(e: &io::Error, segmented: bool) -> Sent {
    if e.kind() == io::ErrorKind::WouldBlock {
        return Sent::Blocked;
    }
    #[cfg(unix)]
    if e.raw_os_error() == Some(libc::EMSGSIZE) {
        return Sent::Probing;
    }
    // quinn-udp's own rule, mirrored: on Linux an adapter that cannot do segmentation offload
    // answers a GSO batch with EIO or EINVAL, and quinn turns GSO off and carries on. Measured on
    // the Android emulator's virtual NIC: one `halting segmentation offload` at startup, which
    // `offload status` then reported for the life of the daemon as the kernel refusing to speak.
    // Only a segmented transmit, because the same errno on a single datagram is a real refusal.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    if segmented && matches!(e.raw_os_error(), Some(libc::EIO | libc::EINVAL)) {
        return Sent::Probing;
    }
    let _ = segmented;
    Sent::Refused
}

/// One destination this machine has refused to send to, and what it said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendRefusal {
    /// Where the datagram was going. An address rather than a [`NodeId`](offload_core::NodeId):
    /// a dial that fails this way may be a seed nobody has identified yet, and the address is
    /// what an operator types into `ping` next.
    pub destination: SocketAddr,
    /// How many the kernel has refused since this daemon started.
    pub refused: u64,
    /// The last thing it said, in the operating system's own words.
    pub last: String,
}

/// The counter behind [`WatchedSocket`], shared with whoever reports it.
#[derive(Debug, Default)]
pub struct SendRefusals {
    seen: Mutex<Counts>,
}

#[derive(Debug, Default)]
struct Counts {
    /// Every refusal, including those past [`REMEMBERED`] destinations.
    total: u64,
    by_destination: HashMap<SocketAddr, (u64, String)>,
}

impl SendRefusals {
    fn record(&self, destination: SocketAddr, e: &io::Error) {
        // A dual-stack socket (`[::]`, the default listen address) hands quinn every IPv4 peer
        // as a v4-mapped address, and `[::ffff:192.0.2.5]:7601` is not the address anybody
        // configured or will recognise — measured on the Mac, the first time a node listened on
        // both families. So the report says the address the way it was written.
        let destination = SocketAddr::new(destination.ip().to_canonical(), destination.port());
        // A poisoned lock here would mean losing the count, which is not worth propagating a
        // panic through a socket send for: the datagram's fate is already decided.
        let Ok(mut seen) = self.seen.lock() else {
            return;
        };
        seen.total = seen.total.saturating_add(1);
        let full = seen.by_destination.len() >= REMEMBERED;
        match seen.by_destination.get_mut(&destination) {
            Some(entry) => {
                entry.0 = entry.0.saturating_add(1);
                entry.1 = e.to_string();
            }
            None if full => {}
            None => {
                seen.by_destination.insert(destination, (1, e.to_string()));
            }
        }
    }

    /// Every refusal this daemon has counted, including destinations past the cap.
    ///
    /// Separate from summing [`Self::by_destination`], because those two numbers can disagree
    /// and the total is the honest one.
    #[must_use]
    pub fn total(&self) -> u64 {
        self.seen.lock().map_or(0, |seen| seen.total)
    }

    /// The breakdown, worst first, then by address so a report is stable between calls.
    #[must_use]
    pub fn by_destination(&self) -> Vec<SendRefusal> {
        let Ok(seen) = self.seen.lock() else {
            return Vec::new();
        };
        let mut out: Vec<SendRefusal> = seen
            .by_destination
            .iter()
            .map(|(destination, (refused, last))| SendRefusal {
                destination: *destination,
                refused: *refused,
                last: last.clone(),
            })
            .collect();
        out.sort_by(|a, b| {
            b.refused
                .cmp(&a.refused)
                .then_with(|| a.destination.to_string().cmp(&b.destination.to_string()))
        });
        out
    }
}

/// quinn's tokio socket, with the errors it discards counted on the way past.
///
/// A near-copy of `quinn::runtime::tokio::UdpSocket`, which is not public. The one difference
/// is [`Self::try_send`]: it calls `UdpSocketState::try_send` rather than `send`, so the error
/// exists to be recorded, and then returns what `send` would have returned. Everything else
/// delegates to the same `UdpSocketState`, so GSO, GRO and MTU behaviour are quinn's.
#[derive(Debug)]
pub struct WatchedSocket {
    io: tokio::net::UdpSocket,
    state: UdpSocketState,
    refusals: Arc<SendRefusals>,
}

impl WatchedSocket {
    /// Take over a bound socket. `refusals` is shared with whoever reports them.
    pub fn new(
        socket: std::net::UdpSocket,
        refusals: Arc<SendRefusals>,
    ) -> io::Result<WatchedSocket> {
        // `UdpSocketState::new` puts the socket in non-blocking mode, which is what
        // `from_std` requires — same order quinn's own runtime does it in.
        let state = UdpSocketState::new((&socket).into())?;
        Ok(WatchedSocket {
            io: tokio::net::UdpSocket::from_std(socket)?,
            state,
            refusals,
        })
    }
}

impl AsyncUdpSocket for WatchedSocket {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        Box::pin(Writable {
            socket: self,
            waiting: None,
        })
    }

    fn try_send(&self, transmit: &Transmit) -> io::Result<()> {
        self.io.try_io(tokio::io::Interest::WRITABLE, || {
            match self.state.try_send((&self.io).into(), transmit) {
                Ok(()) => Ok(()),
                Err(e) => match classify(&e, transmit.segment_size.is_some()) {
                    // Handed back, so quinn registers for writability. Anything else here
                    // would be a datagram silently dropped under load.
                    Sent::Blocked => Err(e),
                    Sent::Probing => Ok(()),
                    Sent::Refused => {
                        self.refusals.record(transmit.destination, &e);
                        Ok(())
                    }
                },
            }
        })
    }

    fn poll_recv(
        &self,
        cx: &mut Context,
        bufs: &mut [io::IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        loop {
            ready!(self.io.poll_recv_ready(cx))?;
            if let Ok(received) = self.io.try_io(tokio::io::Interest::READABLE, || {
                self.state.recv((&self.io).into(), bufs, meta)
            }) {
                return Poll::Ready(Ok(received));
            }
        }
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.io.local_addr()
    }

    fn may_fragment(&self) -> bool {
        self.state.may_fragment()
    }

    fn max_transmit_segments(&self) -> usize {
        self.state.max_gso_segments()
    }

    fn max_receive_segments(&self) -> usize {
        self.state.gro_segments()
    }
}

/// One task's registration for write-readiness. quinn's `UdpPollHelper` is private, so this is
/// the same shape: a future that owns the socket, kept between polls and dropped once ready.
struct Writable {
    socket: Arc<WatchedSocket>,
    waiting: Option<Pin<Box<dyn Future<Output = io::Result<()>> + Send + Sync>>>,
}

impl std::fmt::Debug for Writable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Writable").finish_non_exhaustive()
    }
}

impl UdpPoller for Writable {
    fn poll_writable(self: Pin<&mut Self>, cx: &mut Context) -> Poll<io::Result<()>> {
        // Every field is `Unpin` — the future is behind a `Box` — so the pin projection quinn
        // needs `pin_project_lite` for is `get_mut` here.
        let this = self.get_mut();
        let waiting = match &mut this.waiting {
            Some(waiting) => waiting,
            none => {
                let socket = this.socket.clone();
                none.get_or_insert(Box::pin(async move { socket.io.writable().await }))
            }
        };
        let ready = waiting.as_mut().poll(cx);
        if ready.is_ready() {
            // Polling a future after it is ready is a logic error, so the next call makes a
            // fresh one. A `UdpPoller` is reused indefinitely.
            this.waiting = None;
        }
        ready
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(port: u16) -> SocketAddr {
        SocketAddr::from(([192, 0, 2, 1], port))
    }

    #[test]
    fn a_full_socket_is_not_a_refusal() {
        let would_block = io::Error::from(io::ErrorKind::WouldBlock);
        assert_eq!(classify(&would_block, false), Sent::Blocked);
    }

    #[cfg(unix)]
    #[test]
    fn an_mtu_probe_is_not_a_refusal() {
        let too_big = io::Error::from_raw_os_error(libc::EMSGSIZE);
        assert_eq!(classify(&too_big, false), Sent::Probing);
    }

    #[cfg(unix)]
    #[test]
    fn the_errors_quinn_discards_are_refusals() {
        for errno in [
            libc::EPERM,
            libc::EACCES,
            libc::EHOSTUNREACH,
            libc::ENETUNREACH,
        ] {
            let refused = io::Error::from_raw_os_error(errno);
            assert_eq!(classify(&refused, false), Sent::Refused, "errno {errno}");
        }
    }

    /// An adapter without segmentation offload answers quinn's first GSO batch with EIO, and
    /// quinn falls back. That is quinn learning the NIC, not the kernel refusing to speak — but
    /// only for a batch: the same errno on a single datagram is still a refusal.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn a_gso_fallback_is_not_a_refusal_and_a_single_datagram_error_still_is() {
        for errno in [libc::EIO, libc::EINVAL] {
            let e = io::Error::from_raw_os_error(errno);
            assert_eq!(
                classify(&e, true),
                Sent::Probing,
                "errno {errno}, segmented"
            );
            assert_eq!(classify(&e, false), Sent::Refused, "errno {errno}, single");
        }
    }

    #[test]
    fn a_quiet_machine_reports_nothing() {
        let refusals = SendRefusals::default();
        assert_eq!(refusals.total(), 0);
        assert!(refusals.by_destination().is_empty());
    }

    #[test]
    fn refusals_are_counted_per_destination_worst_first() {
        let refusals = SendRefusals::default();
        let denied = io::Error::from(io::ErrorKind::PermissionDenied);
        refusals.record(addr(1), &denied);
        refusals.record(addr(2), &denied);
        refusals.record(addr(2), &denied);

        assert_eq!(refusals.total(), 3);
        let seen = refusals.by_destination();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0].destination, addr(2));
        assert_eq!(seen[0].refused, 2);
        assert_eq!(seen[1].destination, addr(1));
        assert_eq!(seen[1].refused, 1);
        assert!(!seen[0].last.is_empty());
    }

    #[test]
    fn a_v4_peer_on_a_dual_stack_socket_is_named_as_v4() {
        let refusals = SendRefusals::default();
        let denied = io::Error::from(io::ErrorKind::PermissionDenied);
        let mapped: SocketAddr = "[::ffff:192.0.2.5]:7601".parse().expect("addr");
        refusals.record(mapped, &denied);
        refusals.record("192.0.2.5:7601".parse().expect("addr"), &denied);
        let seen = refusals.by_destination();
        assert_eq!(
            seen.len(),
            1,
            "one peer, whichever way the socket spelled it: {seen:?}"
        );
        assert_eq!(seen[0].destination.to_string(), "192.0.2.5:7601");
        assert_eq!(seen[0].refused, 2);
    }

    #[test]
    fn the_last_thing_the_kernel_said_replaces_the_one_before() {
        let refusals = SendRefusals::default();
        refusals.record(addr(1), &io::Error::from(io::ErrorKind::PermissionDenied));
        refusals.record(addr(1), &io::Error::from(io::ErrorKind::HostUnreachable));
        let seen = refusals.by_destination();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].refused, 2);
        assert_eq!(
            seen[0].last,
            io::Error::from(io::ErrorKind::HostUnreachable).to_string()
        );
    }

    /// The breakdown stops growing and the total does not, which is the point of keeping both.
    #[test]
    fn past_the_cap_the_total_still_rises() {
        let refusals = SendRefusals::default();
        let denied = io::Error::from(io::ErrorKind::PermissionDenied);
        for port in 0..u16::try_from(REMEMBERED).unwrap_or(u16::MAX) + 8 {
            refusals.record(addr(port), &denied);
        }
        assert_eq!(refusals.by_destination().len(), REMEMBERED);
        assert_eq!(refusals.total(), u64::try_from(REMEMBERED).unwrap_or(0) + 8);
    }

    /// A destination already known keeps counting after the cap is reached — otherwise the
    /// breakdown would freeze on whichever peers happened to fail first.
    #[test]
    fn a_known_destination_keeps_counting_after_the_cap() {
        let refusals = SendRefusals::default();
        let denied = io::Error::from(io::ErrorKind::PermissionDenied);
        for port in 0..u16::try_from(REMEMBERED).unwrap_or(u16::MAX) + 8 {
            refusals.record(addr(port), &denied);
        }
        refusals.record(addr(0), &denied);
        let seen = refusals.by_destination();
        assert_eq!(seen[0].destination, addr(0));
        assert_eq!(seen[0].refused, 2);
    }
}
