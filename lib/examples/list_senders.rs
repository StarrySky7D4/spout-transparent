//! cargo run -p spout-transparent --example list_senders --locked
use spout_transparent::{discover_sender_names, read_sender_info};

fn main() -> std::io::Result<()> {
    let names = discover_sender_names()?;
    if names.is_empty() {
        println!("No Spout senders available");
    }
    for name in names {
        match read_sender_info(&name, 0) {
            Ok(info) => println!(
                "{name}: {}x{}, DXGI format {}",
                info.width, info.height, info.format
            ),
            Err(error) => eprintln!("{name}: {error}"),
        }
    }
    Ok(())
}
