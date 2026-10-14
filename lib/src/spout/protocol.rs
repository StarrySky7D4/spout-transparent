//! Spout wire layout and resource identity. Sender names preserve original bytes.
use super::shared_memory::SharedMap;
use std::{
    ffi::{c_void, CString},
    fmt, io,
};

pub(super) const SENDER_NAME_CAPACITY: usize = 256;
pub(super) const SHARED_TEXTURE_INFO_SIZE: usize = 280;

#[derive(Clone, Eq, PartialEq)]
pub struct SenderName(pub(super) Vec<u8>);

impl SenderName {
    /// Build a shared-object name without UTF-8 conversion.
    ///
    /// Names must contain 1–255 bytes and no NUL. Display formatting is lossy for
    /// non-UTF-8 names; [`Self::as_bytes`] always preserves the original bytes.
    pub fn from_bytes(name: impl AsRef<[u8]>) -> io::Result<Self> {
        let name = name.as_ref();
        if name.is_empty() || name.len() >= SENDER_NAME_CAPACITY || name.contains(&0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "sender name must contain 1–255 bytes and no NUL",
            ));
        }
        Ok(Self(name.to_vec()))
    }

    /// Original shared-object name bytes, without a terminating NUL.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub(super) fn from_slot(slot: &[u8]) -> Option<Self> {
        let end = slot
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(slot.len());
        (end > 0).then(|| Self(slot[..end].to_vec()))
    }

    pub fn display_name(&self) -> String {
        String::from_utf8_lossy(&self.0).into_owned()
    }

    pub(super) fn with_suffix(&self, suffix: &[u8]) -> io::Result<CString> {
        let mut name = Vec::with_capacity(self.0.len() + suffix.len());
        name.extend_from_slice(&self.0);
        name.extend_from_slice(suffix);
        CString::new(name)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "sender name contains NUL"))
    }
}

impl std::str::FromStr for SenderName {
    type Err = io::Error;
    fn from_str(name: &str) -> io::Result<Self> {
        Self::from_bytes(name)
    }
}

impl fmt::Debug for SenderName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("SenderName")
            .field(&self.display_name())
            .finish()
    }
}

impl fmt::Display for SenderName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.display_name())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SenderInfo {
    pub share_handle: u32,
    pub width: u32,
    pub height: u32,
    pub format: u32,
    pub usage: u32,
    pub description: String,
    pub partner_id: u32,
}

impl SenderInfo {
    fn parse(bytes: &[u8]) -> io::Result<Self> {
        if bytes.len() < SHARED_TEXTURE_INFO_SIZE {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Spout sender metadata is truncated",
            ));
        }

        let read_u32 = |offset: usize| {
            u32::from_le_bytes(
                bytes[offset..offset + 4]
                    .try_into()
                    .expect("checked length"),
            )
        };
        let description_bytes = &bytes[20..276];
        let description_end = description_bytes
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(description_bytes.len());

        Ok(Self {
            share_handle: read_u32(0),
            width: read_u32(4),
            height: read_u32(8),
            format: read_u32(12),
            usage: read_u32(16),
            description: String::from_utf8_lossy(&description_bytes[..description_end])
                .into_owned(),
            partner_id: read_u32(276),
        })
    }

    pub(super) fn is_usable(&self) -> bool {
        self.share_handle != 0 && self.share_handle != u32::MAX && self.width > 0 && self.height > 0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SenderIdentity {
    pub name: SenderName,
    pub share_handle: u32,
    pub generation: u64,
}

impl SenderIdentity {
    pub fn raw_handle(&self) -> *mut c_void {
        self.share_handle as i32 as isize as *mut c_void
    }
}

/// Read the sender's 280-byte metadata under its named mutex.
///
/// `timeout_ms = 0` is nonblocking. A contended mutex returns `WouldBlock`;
/// a missing sender returns `NotFound`. Avoid nonzero timeouts on a render thread.
pub fn read_sender_info(sender: &SenderName, timeout_ms: u32) -> io::Result<SenderInfo> {
    let map = SharedMap::open(&sender.0)?;
    let mut bytes = [0; SHARED_TEXTURE_INFO_SIZE];
    let len = map.read_into(&mut bytes, timeout_ms)?;
    SenderInfo::parse(&bytes[..len])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncated_metadata_is_rejected_without_reading_past_the_buffer() {
        for length in [0, 20, SHARED_TEXTURE_INFO_SIZE - 1] {
            let error = SenderInfo::parse(&vec![0; length]).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
        }
    }

    #[test]
    fn non_utf8_sender_names_preserve_the_shared_object_name() {
        let name = SenderName::from_slot(&[0xff, b'A', 0, b'B']).unwrap();
        assert_eq!(
            name.with_suffix(b"_mutex").unwrap().as_bytes(),
            &[0xff, b'A', b'_', b'm', b'u', b't', b'e', b'x']
        );
        assert_eq!(name.display_name(), "\u{fffd}A");
    }
    #[test]
    fn parses_sender_names_until_empty_slot() {
        let mut bytes = vec![0; SENDER_NAME_CAPACITY * 3];
        bytes[..5].copy_from_slice(b"Alpha");
        bytes[SENDER_NAME_CAPACITY..SENDER_NAME_CAPACITY + 4].copy_from_slice(b"Beta");

        let names: Vec<_> = bytes
            .chunks_exact(SENDER_NAME_CAPACITY)
            .map_while(SenderName::from_slot)
            .map(|name| name.display_name())
            .collect();
        assert_eq!(names, ["Alpha", "Beta"]);
    }

    #[test]
    fn parses_spout_shared_texture_info_layout() {
        let mut bytes = [0u8; SHARED_TEXTURE_INFO_SIZE];
        bytes[0..4].copy_from_slice(&0x89ab_cdefu32.to_le_bytes());
        bytes[4..8].copy_from_slice(&1920u32.to_le_bytes());
        bytes[8..12].copy_from_slice(&1080u32.to_le_bytes());
        bytes[12..16].copy_from_slice(&87u32.to_le_bytes());
        bytes[16..20].copy_from_slice(&1u32.to_le_bytes());
        bytes[20..25].copy_from_slice(b"alpha");
        bytes[276..280].copy_from_slice(&42u32.to_le_bytes());

        let info = SenderInfo::parse(&bytes).unwrap();
        assert_eq!(info.share_handle, 0x89ab_cdef);
        assert_eq!((info.width, info.height), (1920, 1080));
        assert_eq!(info.format, 87);
        assert_eq!(info.usage, 1);
        assert_eq!(info.description, "alpha");
        assert_eq!(info.partner_id, 42);
    }

    #[test]
    fn sign_extends_spout_handle_on_64_bit_targets() {
        let info = SenderInfo {
            share_handle: 0x89ab_cdef,
            width: 1,
            height: 1,
            format: 0,
            usage: 0,
            description: String::new(),
            partner_id: 0,
        };
        let identity = SenderIdentity {
            name: SenderName::from_bytes("source").unwrap(),
            share_handle: info.share_handle,
            generation: 1,
        };
        assert_eq!(
            identity.raw_handle() as isize,
            0x89ab_cdefu32 as i32 as isize
        );
    }
}
