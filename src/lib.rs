#![doc=include_str!("../README.md")]

use atomic_waker::AtomicWaker;
use nix::libc::c_int;
use nix::sys::signal::sigaction;
use nix::sys::signal::SaFlags;
use nix::sys::signal::SigAction;
use nix::sys::signal::SigHandler;
use nix::sys::signal::SigSet;
pub use nix::sys::signal::Signal;
use once_cell::sync::Lazy;
use std::cell::UnsafeCell;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::sync::Once;
use std::task::Context;
use std::task::Poll;

/// The inner structure of our `SignalFut`.
struct FutInner {
    /// Corresponding Signal
    signal: Signal,
    /// The task waker
    waker: AtomicWaker,
    /// Has 1 pending signal
    pending: AtomicBool,
}

impl FutInner {
    /// Create a `FutInner` that has no `waker` and pending signal.
    fn new(signal: usize) -> Self {
        Self {
            signal: Signal::try_from(signal as i32).expect("invalid signal number"),
            waker: AtomicWaker::new(),
            pending: AtomicBool::new(false),
        }
    }
}

/// A future that would be resolved when received the specified signal.
///
/// # Examples
///
/// Wait for `SIGTERM`:
///
/// ```rust,no_run
/// # use monoio::FusionDriver;
/// # use nix::sys::signal::Signal;
/// # use async_signal_handler::SignalFut;
/// #
/// # async {
/// let sigterm_fut = SignalFut::new(Signal::SIGTERM);
/// sigterm_fut.await;
/// # };
/// ```
#[derive(Clone)]
pub struct SignalFut(Arc<FutInner>);

impl SignalFut {
    pub fn new(signal: Signal) -> Self {
        let sig_num = signal as usize;

        // This signal has already been registered
        if REGISTERED_SIGNALS[sig_num].0.load(Ordering::Relaxed) {
            let fut = SignalFut::clone(unsafe { REGISTERED_SIGNALS[sig_num].2.get() });
            return fut;
        }

        // need to register it
        REGISTERED_SIGNALS[sig_num].1.call_once(|| {
            let fut = SignalFut(Arc::new(FutInner::new(sig_num)));
            unsafe { REGISTERED_SIGNALS[sig_num].2.set(fut) };

            // dispose the signal
            let sig_handler = SigHandler::Handler(handler);
            let sig_action = SigAction::new(sig_handler, SaFlags::empty(), SigSet::empty());
            // SAFETY:
            unsafe { sigaction(signal, &sig_action).unwrap() };

            // Set the initialized mark
            REGISTERED_SIGNALS[sig_num].0.store(true, Ordering::Relaxed);
        });

        if !REGISTERED_SIGNALS[sig_num].0.load(Ordering::Relaxed) {
            panic!("failed to register the signal");
        }

        SignalFut::clone(unsafe { REGISTERED_SIGNALS[sig_num].2.get() })
    }

    /// Return the inner `Signal`.
    pub fn signal(&self) -> Signal {
        self.0.signal
    }

    /// Will be invoked when the corresponding signal is received.
    ///
    /// # Signal-safe
    ///
    /// This function has to be signal-safe because it will be used in the signal handler.
    fn add_signal(&self) {
        self.0.pending.store(true, Ordering::Relaxed);
        self.0.waker.wake();
    }
}

impl Future for SignalFut {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.0.waker.register(cx.waker());

        if self.0.pending.load(Ordering::Relaxed) {
            self.0.pending.store(false, Ordering::Relaxed);
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}

/// Define a wrapper type for `UnsafeCell<Option<T>>` so that we can impl `Sync`
/// for it.
struct UnsafeOption<T>(UnsafeCell<Option<T>>);

impl<T> UnsafeOption<T> {
    /// Create an `UnsafeOption` and set it to `None`.
    fn none() -> Self {
        Self(UnsafeCell::new(None))
    }

    /// Set the inner `T` value.
    ///
    /// # Safety
    ///
    /// 1. You have to ensure no one is reading this through `.get()`.
    ///
    /// # Dirty write
    ///
    /// When calling concurrently from multiple threads, the inner `T` value can be
    /// incomplete.
    unsafe fn set(&self, val: T) {
        let ptr = self.0.get();
        *ptr = Some(val);
    }

    /// Get a reference to the inner `T`.
    ///
    /// # Safety
    ///
    /// 1. You have to ensure tha the inner `Option` is `Some`.
    /// 2. You have to ensure the write operation to the inner `T` is complete.
    /// 2. You have to ensure that no one is `.set()`ting it during the lifetime
    ///    of the returned `&T`.
    unsafe fn get(&self) -> &T {
        let ptr = self.0.get();
        (*ptr).as_ref().expect("should be Some")
    }
}

/// SAFETY:
unsafe impl<T: Sync> Sync for UnsafeOption<T> {}

static REGISTERED_SIGNALS: Lazy<Vec<(AtomicBool, Once, UnsafeOption<SignalFut>)>> =
    Lazy::new(|| {
        let n_signal = nix::libc::SIGRTMAX() as usize;
        (0..n_signal)
            .map(|_| (AtomicBool::new(false), Once::new(), UnsafeOption::none()))
            .collect()
    });

/// The signal handler.
extern "C" fn handler(sig_num: c_int) {
    let sig_num = sig_num as usize;
    if REGISTERED_SIGNALS[sig_num].0.load(Ordering::Relaxed) {
        let fut = unsafe { REGISTERED_SIGNALS[sig_num].2.get() };
        fut.add_signal();
    } else {
        panic!()
    }
}

/// Completes when a “ctrl-c” notification is sent to the process.
pub async fn ctrl_c() {
    SignalFut::new(Signal::SIGINT).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use nix::sys::signal::kill;
    use nix::unistd::Pid;

    #[monoio::test]
    async fn sigint_with_monoio() {
        let fut = SignalFut::new(Signal::SIGINT);
        kill(Pid::this(), Signal::SIGINT).unwrap();
        fut.await;
    }

    #[tokio::test]
    async fn sigint_with_tokio() {
        let fut = SignalFut::new(Signal::SIGINT);
        kill(Pid::this(), Signal::SIGINT).unwrap();
        fut.await;
    }

    #[test]
    fn signt_with_futures_block_on() {
        futures::executor::block_on(async move {
            let fut = SignalFut::new(Signal::SIGINT);
            kill(Pid::this(), Signal::SIGINT).unwrap();
            fut.await;
        });
    }

    #[test]
    fn sigint_with_glommio() {
        glommio::LocalExecutorBuilder::default()
            .spawn(|| async {
                let fut = SignalFut::new(Signal::SIGINT);
                kill(Pid::this(), Signal::SIGINT).unwrap();
                fut.await;
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[tokio::test]
    async fn multiple_futures_with_tokio_select() {
        let sigint_fut = SignalFut::new(Signal::SIGINT);
        let sigterm_fut = SignalFut::new(Signal::SIGTERM);
        kill(Pid::this(), Signal::SIGTERM).unwrap();
        tokio::select! {
            _ = sigint_fut => {},
            _ = sigterm_fut => {},
        }
    }

    #[tokio::test]
    async fn multiple_tasks_waiting_for_same_signal() {
        let task1 = async {
            SignalFut::new(Signal::SIGINT).await;
        };
        let task2 = async {
            SignalFut::new(Signal::SIGINT).await;
        };

        let handle1 = tokio::spawn(task1);
        let handle2 = tokio::spawn(task2);

        // sleep for 1 second to ensure that task1 and task2 will be polled for
        // at least once so that signal handler can be set.
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        kill(Pid::this(), Signal::SIGINT).unwrap();

        handle1.await.unwrap();
        handle2.await.unwrap();
    }
}
