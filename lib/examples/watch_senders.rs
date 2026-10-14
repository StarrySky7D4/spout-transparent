//! cargo run -p spout-transparent --example watch_senders --locked
//! Prints only changes; Ctrl+C ends this console example.
use spout_transparent::SenderDiscovery;
use std::sync::mpsc;

fn main() -> std::io::Result<()> {
    let (notify, events) = mpsc::channel();
    let discovery = SenderDiscovery::start(move || notify.send(()).is_ok())?;
    for () in events {
        if let Some(snapshot) = discovery.take_latest() {
            match snapshot {
                Ok(names) => println!("Senders: {names:?}"),
                Err(error) => eprintln!("Discovery failed: {error}"),
            }
        }
    }
    Ok(())
}
