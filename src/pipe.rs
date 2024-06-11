/// Return a pipe, with `CLOEXEC` set.
use std::os::fd::OwnedFd;

/// Return a pipe, with `O_CLOEXEC` set.
#[cfg(target_os = "linux")]
pub(crate) fn pipe() -> (OwnedFd, OwnedFd) {
    nix::unistd::pipe2(nix::fcntl::OFlag::O_CLOEXEC).unwrap()
}

/// Return a pipe, with `FD_CLOEXEC` set.
#[cfg(target_os = "macos")]
pub(crate) fn pipe() -> (OwnedFd, OwnedFd) {
    use nix::fcntl::fcntl;
    use nix::fcntl::FcntlArg;
    use nix::fcntl::FdFlag;
    use nix::unistd::pipe;

    let (rx, tx) = pipe().unwrap();

    fcntl(&rx, FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC)).unwrap();
    fcntl(&tx, FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC)).unwrap();

    (rx, tx)
}
