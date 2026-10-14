//! Independent benchmark process linking the same library used by the application.
mod support;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    support::measurements::measure_performance()
}
