## Async signal handler

Similar to [tokio::signal][link], but can be used with [glommio][g] and [monoio][m].

[link]: https://docs.rs/tokio/latest/tokio/signal/index.html
[g]: https://github.com/DataDog/glommio
[m]: https://github.com/bytedance/monoio

## Examples

Print on “ctrl-c” notification.

```rust,no_run
use async_signal_handler::ctrl_c;
use monoio::FusionDriver;

fn main() {
    let mut rt = monoio::RuntimeBuilder::<FusionDriver>::new()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async move {
        loop {
            ctrl_c().await;
            println!("SIGINT received");
        }
    });
}
```
