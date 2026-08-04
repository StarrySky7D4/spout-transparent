use autocxx::{c_int, c_uint};
use rust_spout2::Spout;
use std::ffi::{c_char, CString};
use std::thread;
use std::time::Duration;

const SPOUTLIBRARY_DLL: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/SpoutLibrary.dll"));
const SPOUT_CONNECT_MAX_RETRIES: u32 = 100;
const SPOUT_CONNECT_RETRY_INTERVAL_MS: u64 = 50;

pub fn extract_spout_dll() -> Result<(), std::io::Error> {
    let exe_dir = std::env::current_exe()
        .map_err(|e| {
            log::error!("Failed to get exe path: {e}");
            e
        })?
        .parent()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "No parent dir"))?
        .to_path_buf();
    let dll_path = exe_dir.join("SpoutLibrary.dll");
    if !dll_path.exists() {
        std::fs::write(&dll_path, SPOUTLIBRARY_DLL)?;
    }
    Ok(())
}

pub fn list_senders(spout: &mut Spout) -> Vec<String> {
    let count: i32 = spout.as_pin_mut().GetSenderCount().into();
    let mut senders = Vec::new();
    for i in 0..count {
        let mut buf = [0u8; 256];
        let found = unsafe {
            spout
                .as_pin_mut()
                .GetSender(c_int(i), buf.as_mut_ptr() as *mut c_char, c_int(256))
        };
        if !found {
            continue;
        }
        let end = buf.iter().position(|byte| *byte == 0).unwrap_or(buf.len());
        let name = String::from_utf8_lossy(&buf[..end]).into_owned();
        if !name.is_empty() {
            senders.push(name);
        }
    }
    senders
}

pub fn spout_connect(spout: &mut Spout, sender_name: &str) -> Result<bool, std::ffi::NulError> {
    let cname = CString::new(sender_name)?;
    unsafe { spout.as_pin_mut().SetReceiverName(cname.as_ptr()) };
    for _ in 0..SPOUT_CONNECT_MAX_RETRIES {
        if spout
            .as_pin_mut()
            .ReceiveTexture(c_uint(0), c_uint(0), true, c_uint(0))
        {
            return Ok(true);
        }
        thread::sleep(Duration::from_millis(SPOUT_CONNECT_RETRY_INTERVAL_MS));
    }
    Ok(false)
}
