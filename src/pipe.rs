/// Return a pipe, with `O_CLOEXEC` set.
use std::os::fd::OwnedFd;

/// Return a pipe, with `O_CLOEXEC` set.
#[cfg(target_os = "linux")]
pub(crate) fn pipe() -> (OwnedFd, OwnedFd) {
    nix::unistd::pipe2(nix::fcntl::OFlag::O_CLOEXEC).unwrap()
}

/// Return a pipe, with `O_CLOEXEC` set.
#[cfg(target_os = "macos")]
pub(crate) fn pipe() -> (OwnedFd, OwnedFd) {
    use std::os::fd::AsRawFd;

    let (rx, tx) = nix::unistd::pipe().unwrap();
    let mut rx_flag = nix::fcntl::OFlag::from_bits(
        nix::fcntl::fcntl(rx.as_raw_fd(), nix::fcntl::FcntlArg::F_GETFL).unwrap(),
    )
    .unwrap();
    rx_flag.insert(nix::fcntl::OFlag::O_CLOEXEC);
    let mut tx_flag = nix::fcntl::OFlag::from_bits(
        nix::fcntl::fcntl(tx.as_raw_fd(), nix::fcntl::FcntlArg::F_GETFL).unwrap(),
    )
    .unwrap();
    tx_flag.insert(nix::fcntl::OFlag::O_CLOEXEC);

    nix::fcntl::fcntl(rx.as_raw_fd(), nix::fcntl::FcntlArg::F_SETFL(rx_flag)).unwrap();
    nix::fcntl::fcntl(tx.as_raw_fd(), nix::fcntl::FcntlArg::F_SETFL(tx_flag)).unwrap();

    (rx, tx)
}
