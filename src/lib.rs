#![doc=include_str!("../README.md")]

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
use std::task::Context;
use std::task::Poll;

#[derive(Debug)]
struct UnsafeCellOptEvent(UnsafeCell<Option<Event>>);

unsafe impl Send for UnsafeCellOptEvent {}
unsafe impl Sync for UnsafeCellOptEvent {}

impl UnsafeCellOptEvent {
    fn new() -> Self {
        UnsafeCellOptEvent(UnsafeCell::new(None))
    }

    fn get(&self) -> &Option<Event> {
        unsafe { &*self.0.get() }
    }

    fn set(&self, event: Event) {
        let ptr = self.0.get();
        unsafe {
            *ptr = Some(event);
        }
    }
}

static REGISTERED_EVENTS: Lazy<Vec<(AtomicBool, UnsafeCellOptEvent)>> = Lazy::new(|| {
    let n_signal = nix::libc::SIGRTMAX() as usize;
    (0..n_signal)
        .map(|_| (AtomicBool::new(false), UnsafeCellOptEvent::new()))
        .collect()
});

extern "C" fn handler(sig_num: c_int) {
    let sig_num = sig_num as usize;
    debug_assert!(REGISTERED_EVENTS[sig_num].0.load(Ordering::Relaxed));
    REGISTERED_EVENTS
        .get(sig_num)
        .expect("all the signal numbers should be smaller than libc::SIGRTMAX")
        .1
        .get()
        .as_ref()
        .expect("event should be set")
        .notify(usize::MAX);
}

pub use nix::sys::signal::Signal;

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
        let has_event = REGISTERED_EVENTS
            .get(sig_num)
            .expect("all the signal numbers should be smaller than libc::SIGRTMAX")
            .0
            .load(Ordering::Relaxed);

        if has_event {
            let listener = REGISTERED_EVENTS
                .get(sig_num)
                .expect("")
                .1
                .get()
                .as_ref()
                .expect("")
                .listen();
            SignalFut { signal, listener }
        } else {
            // do the job
            let event = Event::new();
            let listener = event.listen();
            REGISTERED_EVENTS.get(sig_num).expect("").1.set(event);
            let sig_handler = SigHandler::Handler(handler);
            let sig_action = SigAction::new(sig_handler, SaFlags::empty(), SigSet::empty());
            // SAFETY: let's just assume it is safe
            unsafe { sigaction(signal, &sig_action).unwrap() };

            // then race
            let res = REGISTERED_EVENTS
                .get(sig_num)
                .expect("")
                .0
                .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed);

            match res {
                Ok(_) => SignalFut { signal, listener },
                Err(_) => {
                    let listener = REGISTERED_EVENTS
                        .get(sig_num)
                        .expect("")
                        .1
                        .get()
                        .as_ref()
                        .expect("")
                        .listen();
                    SignalFut { signal, listener }
                }
            }
        }
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
    async fn sighup_with_monoio() {
        let fut = SignalFut::new(Signal::SIGHUP);
        kill(Pid::this(), Signal::SIGHUP).unwrap();
        fut.await;
    }

    #[tokio::test]
    async fn sigint_with_tokio() {
        let fut = SignalFut::new(Signal::SIGINT);
        kill(Pid::this(), Signal::SIGINT).unwrap();
        fut.await;
    }

    #[test]
    fn sigquit_with_futures_block_on() {
        futures::executor::block_on(async move {
            let fut = SignalFut::new(Signal::SIGQUIT);
            kill(Pid::this(), Signal::SIGQUIT).unwrap();
            fut.await;
        });
    }

    #[test]
    fn sigill_with_glommio() {
        glommio::LocalExecutorBuilder::default()
            .spawn(|| async {
                let fut = SignalFut::new(Signal::SIGILL);
                kill(Pid::this(), Signal::SIGILL).unwrap();
                fut.await;
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[tokio::test]
    async fn multiple_futures_with_tokio_select() {
        let sigint_fut = SignalFut::new(Signal::SIGTERM);
        let sigquit_fut = SignalFut::new(Signal::SIGABRT);
        kill(Pid::this(), Signal::SIGTERM).unwrap();
        tokio::select! {
            _ = sigint_fut => {},
            _ = sigquit_fut => {},
        }
    }

    #[tokio::test]
    async fn multiple_tasks_waiting_for_same_signal() {
        let task1 = async {
            SignalFut::new(Signal::SIGURG).await;
        };
        let task2 = async {
            SignalFut::new(Signal::SIGURG).await;
        };

        let handle1 = tokio::spawn(task1);
        let handle2 = tokio::spawn(task2);

        // sleep for 1 second to ensure that task1 and task2 will be polled for
        // at least once so that signal handler can be set.
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        kill(Pid::this(), Signal::SIGURG).unwrap();

        handle1.await.unwrap();
        handle2.await.unwrap();
    }
}
