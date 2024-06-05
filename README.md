## signal-future

A future similar to [tokio::signal::unix::Signal][link], but can be used with:

[link]: https://docs.rs/tokio/latest/tokio/signal/unix/struct.Signal.html#

* Tokio
* async-std
* futures
* smol
* Monoio
* Glommio
* Compio

## Supported platforms

Currently, only Linux and macOS are supported. Windows will also be supported in
the future.

## Examples

Greet when receive either `SIGINT` or `SIGQUIT`:

```rust,no_run
use signal_future::ctrl_c;
use signal_future::Signal;
use signal_future::SignalFut;

#[tokio::main]
async fn main() {
    tokio::select! {
        _ = ctrl_c() => {},
        _ = SignalFut::new(Signal::SIGQUIT) => {},
    }
    println!("Greeting!");
}
```

Let multiple tasks wait for the same signal:

```rust,no_run
use signal_future::ctrl_c;

#[tokio::main]
async fn main() {
    let fut1 = ctrl_c();
    let fut2 = ctrl_c();

    tokio::join!(fut1, fut2);
}
```

## Signal handler

This crate disposes a signal handler for the signals you want to watch, since
signal handler is shared by the whole process, don't use other crates that also
do this, they will clash.

After disposition, the default signal handler will not be reset.

## Helper thread and Signal Safety

This crate creates a helper thread to execute the code that is not signal-safe,
the signal handler and the helper thread communicate through an OS pipe, the only
thing that the signal handler does is `write(2)`, which is signal-safe.