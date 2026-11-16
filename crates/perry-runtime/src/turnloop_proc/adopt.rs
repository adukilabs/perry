//! Handing an already-created descriptor to the loop.
//!
//! P2 does not re-create the descriptors it moves. A dgram socket carries
//! Node's bind-time `SO_REUSEADDR`/`SO_REUSEPORT` choice and, afterwards, its
//! multicast membership and interface state; a child's pipes come out of a
//! `std::process::Command` whose `pre_exec` hooks turnloop's `ProcessSpec` has
//! no equivalent for. Re-creating either would mean re-deriving syscalls the
//! existing code already gets right, on the same commit that moves the wait.
//!
//! So the descriptor is created exactly as before and *adopted*:
//! `Detached::from_fd` on Unix, `from_socket` / `from_handle` on Windows,
//! then [`turnloop::Loop::attach`]. turnloop classifies it itself (socket
//! family and type via `getsockname` + `SO_TYPE`, otherwise `fstat` +
//! `isatty`), which is why nothing here has to tell it what kind of
//! descriptor it is getting.
//!
//! # Ownership
//!
//! Adoption **moves** the descriptor. The `OwnedFd`/`OwnedSocket`/`OwnedHandle`
//! is consumed, and from then on the driver closes it — so a caller that still
//! needs the descriptor for something the loop does not expose (dgram's
//! `setsockopt` surface) must hand over a **duplicate** and keep the original,
//! which is what [`crate::dgram_reactor`] does. `dup(2)` shares one open file
//! description, so an option set through the retained copy is the same socket
//! the driver is receiving on; `getsockname` through it answers about the same
//! binding. What it must *not* do is read or write there: turnloop sets
//! `O_NONBLOCK` on the description it adopts, and the duplicate sees that too.

use turnloop::Result as TlResult;

/// A descriptor on its way to the loop, in the form its platform names.
pub(crate) enum Transport {
    /// A Unix descriptor of any kind: pipe, socket, tty or file.
    Fd(std::os::fd::OwnedFd),
}

impl Transport {
    pub(crate) fn into_detached(self) -> TlResult<turnloop::Detached> {
        match self {
            Transport::Fd(fd) => turnloop::Detached::from_fd(fd),
        }
    }
}

/// Duplicate a borrowed descriptor so the loop can own one copy while the
/// caller keeps the other.
///
/// The duplicate shares the open file description, which is exactly what makes
/// it useful and exactly what makes it dangerous: options and the binding are
/// shared (wanted), and so is `O_NONBLOCK` (which is why the retained copy is
/// for `setsockopt`/`getsockname` only, never for I/O).
// Datagram-side surface of the P2 handle table: real, exercised by
// `turnloop_proc::tests`, and consumed in production only by
// `dgram_reactor`, which the `mod-dgram` install brings in.
pub(crate) fn duplicate_fd(fd: std::os::fd::BorrowedFd<'_>) -> std::io::Result<Transport> {
    use std::os::fd::AsRawFd;
    // SAFETY: `fd` is a live borrowed descriptor for the duration of the call,
    // and F_DUPFD_CLOEXEC returns a new owned descriptor or -1.
    let raw = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
    if raw < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `raw` is a fresh descriptor this call owns and nothing else holds.
    Ok(Transport::Fd(unsafe {
        <std::os::fd::OwnedFd as std::os::fd::FromRawFd>::from_raw_fd(raw)
    }))
}

