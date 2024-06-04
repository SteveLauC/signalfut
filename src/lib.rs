#![doc=include_str!("../README.md")]
#![cfg(unix)]
#![deny(clippy::undocumented_unsafe_blocks)]
#![deny(unused)]

use event_listener::Event;
use event_listener::EventListener;
use nix::libc::c_int;
use nix::sys::signal::sigaction;
use nix::sys::signal::SaFlags;
use nix::sys::signal::SigAction;
use nix::sys::signal::SigHandler;
use nix::sys::signal::SigSet;
use once_cell::sync::Lazy;
use pin_project::pin_project;
use std::cell::UnsafeCell;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::Once;
use std::task::Context;
use std::task::Poll;

pub use nix::sys::signal::Signal;

/// A wrapper type for `UnsafeCell<Option<T>>` so that we can impl `Sync`
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
    /// 2. When calling concurrently from multiple threads, the inner `T` value
    ///    can be deterministic.
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
///
/// In this crate's usage, we:
///
/// 1. Write the variable (by ONLY 1 thread, controlled by `std::sync::Once`)
/// 2. Set the atomic flag, which means from now on, we can read the variable
/// 3. Only read the variable during the remaining lifetime
///
/// So it should be safe to mark this type `Sync`.
unsafe impl<T: Sync> Sync for UnsafeOption<T> {}

/// A event that may or may not be registered.
struct RegisteredEvent {
    /// An atomic flag indicating if this event has been registered.
    registered: AtomicBool,
    /// Use the `std::sync::Once` to ensure that ONLY one thread will successfully
    /// register the event when there are multiple threads doing it concurrently.
    ///
    /// From the doc:
    ///
    /// > This method will block the calling thread if another initialization
    /// > routine is currently running.
    ///
    /// so it can be blocking, but shouldn't block for a long time, which is
    /// acceptable.
    once: Once,
    /// Will be `Some(event_listener::Event)` after registration.
    event: UnsafeOption<Event>,
}

impl RegisteredEvent {
    /// Create a `RegisteredEvent` that has not been registered.
    fn unregistered() -> Self {
        Self {
            registered: AtomicBool::new(false),
            once: Once::new(),
            event: UnsafeOption::none(),
        }
    }
}

/// An fixed-length array of `RegisteredEvent`.
///
/// We use the signal number (start from 1) as the index.
static REGISTERED_EVENTS: Lazy<Vec<RegisteredEvent>> = Lazy::new(|| {
    #[cfg(target_os = "linux")]
    let n_signal = nix::libc::SIGRTMAX() as usize;
    #[cfg(all(unix, not(target_os = "linux")))]
    let n_signal = 33;

    (0..=n_signal)
        .map(|_| RegisteredEvent::unregistered())
        .collect()
});

/// The signal handler
///
/// # Signal-safety
///
/// This handler should be signal-safe if `event_listener::Event::notify()` is
/// signal safe.
extern "C" fn handler(sig_num: c_int) {
    let sig_num = sig_num as usize;

    if !REGISTERED_EVENTS[sig_num]
        .registered
        .load(Ordering::Relaxed)
    {
        unreachable!("Event is not registered, but the signal disposition works, which should be impossible.");
    }

    // SAFETY:
    //
    // 1. The value should be `Some` and complete since we read it after checking
    //    the `registered` sign.
    // 2. No one will `.set()` it, we only set it once.
    unsafe {
        REGISTERED_EVENTS[sig_num].event.get().notify(usize::MAX);
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
/// # use signal_future::SignalFut;
/// #
/// # async {
/// let sigterm_fut = SignalFut::new(Signal::SIGTERM);
/// sigterm_fut.await;
/// # };
/// ```
#[pin_project]
pub struct SignalFut {
    signal: Signal,
    #[pin]
    listener: EventListener,
}

impl SignalFut {
    /// Create a `SignalFut` for `signal`.
    pub fn new(signal: Signal) -> SignalFut {
        let sig_num = signal as usize;
        if REGISTERED_EVENTS[sig_num]
            .registered
            .load(Ordering::Relaxed)
        {
            // SAFETY:
            //
            // 1. The value should be `Some` and complete since we read it after checking
            //    the `registered` sign.
            // 2. No one will `.set()` it, we only set it once.
            let event = unsafe { REGISTERED_EVENTS[sig_num].event.get() };
            let listener = event.listen();
            return SignalFut { signal, listener };
        }

        REGISTERED_EVENTS[sig_num].once.call_once(|| {
            // Create event
            let event = Event::new();
            // SAFETY:
            //
            // 1. No one is reading this because the `registered` sign is still `false`
            // 2. No concurrent calls exist (guarded by `std::sync::Once`)
            unsafe {
                REGISTERED_EVENTS[sig_num].event.set(event);
            }

            // dispose the signal
            let sig_handler = SigHandler::Handler(handler);
            let sig_action = SigAction::new(sig_handler, SaFlags::empty(), SigSet::empty());
            // SAFETY:
            // if `event-listener::Event::notify(usize::MAX)` is signal-safe, then it is safe.
            unsafe { sigaction(signal, &sig_action).unwrap() };

            // Set the initialized mark
            REGISTERED_EVENTS[sig_num]
                .registered
                .store(true, Ordering::Relaxed);
        });

        if !REGISTERED_EVENTS[sig_num]
            .registered
            .load(Ordering::Relaxed)
        {
            panic!("failed to set signal handler");
        }

        // SAFETY:
        //
        // 1. The value should be `Some` and complete since we read it after checking
        //    the `registered` sign.
        // 2. No one will `.set()` it, we only set it once.
        let event = unsafe { REGISTERED_EVENTS[sig_num].event.get() };
        let listener = event.listen();
        SignalFut { signal, listener }
    }
}

impl Future for SignalFut {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        this.listener.poll(cx)
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
    #[cfg(target_os = "linux")] // glommio is Linux-only
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
        let sigquit_fut = SignalFut::new(Signal::SIGTERM);
        kill(Pid::this(), Signal::SIGTERM).unwrap();
        tokio::select! {
            _ = sigint_fut => {},
            _ = sigquit_fut => {},
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
