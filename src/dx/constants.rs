#[allow(dead_code)]
pub const DXGI_FORMAT_R8G8B8A8_UNORM: i32 = 28;
#[allow(dead_code)]
pub const DXGI_FORMAT_R8G8B8A8_UNORM_SRGB: i32 = 29;
#[allow(dead_code)]
pub const DXGI_FORMAT_B8G8R8A8_UNORM: i32 = 87;
#[allow(dead_code)]
pub const DXGI_FORMAT_B8G8R8A8_UNORM_SRGB: i32 = 91;

pub const BPP_RGBA: usize = 4;

pub const EXSTYLE_LAYERED: i32 = 0x00080000;
pub const EXSTYLE_TRANSPARENT: i32 = 0x00000020;
pub const EXSTYLE_NOREDIRECTIONBITMAP: i32 = 0x00200000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum PixelFormat {
    R8G8B8A8,
    B8G8R8A8,
    Unknown,
}

#[allow(dead_code)]
impl PixelFormat {
    pub fn from_dxgi(code: i32) -> Self {
        match code {
            DXGI_FORMAT_R8G8B8A8_UNORM | DXGI_FORMAT_R8G8B8A8_UNORM_SRGB => PixelFormat::R8G8B8A8,
            DXGI_FORMAT_B8G8R8A8_UNORM | DXGI_FORMAT_B8G8R8A8_UNORM_SRGB => PixelFormat::B8G8R8A8,
            _ => PixelFormat::Unknown,
        }
    }

    pub fn channel_offset_r(self) -> usize {
        match self {
            PixelFormat::R8G8B8A8 => 0,
            PixelFormat::B8G8R8A8 => 2,
            PixelFormat::Unknown => 0,
        }
    }

    pub fn channel_offset_b(self) -> usize {
        match self {
            PixelFormat::R8G8B8A8 => 2,
            PixelFormat::B8G8R8A8 => 0,
            PixelFormat::Unknown => 2,
        }
    }

    pub fn channel_offset_g(self) -> usize {
        1
    }

    pub fn channel_offset_a(self) -> usize {
        3
    }
}

#[allow(dead_code)]
pub fn extract_channel_rgba(
    src: &[u8],
    src_pitch: u32,
    _src_format: PixelFormat,
    width: u32,
    height: u32,
    channel: usize,
) -> Vec<u8> {
    let mut out = Vec::with_capacity((width * height) as usize);
    for y in 0..height {
        let row_off = (y * src_pitch) as usize;
        for x in 0..width {
            let px = row_off + (x as usize) * BPP_RGBA;
            if px + BPP_RGBA <= src.len() {
                out.push(src[px + channel]);
            }
        }
    }
    out
}

#[allow(dead_code)]
pub fn convert_row_to_bgra(
    src: &[u8],
    src_format: PixelFormat,
    dst: &mut [u8],
    pixel_count: usize,
) {
    for i in 0..pixel_count {
        let si = i * BPP_RGBA;
        let di = i * BPP_RGBA;
        if si + BPP_RGBA > src.len() || di + BPP_RGBA > dst.len() {
            break;
        }
        match src_format {
            PixelFormat::R8G8B8A8 => {
                dst[di] = src[si + 2];
                dst[di + 1] = src[si + 1];
                dst[di + 2] = src[si];
                dst[di + 3] = src[si + 3];
            }
            PixelFormat::B8G8R8A8 => {
                dst[di] = src[si];
                dst[di + 1] = src[si + 1];
                dst[di + 2] = src[si + 2];
                dst[di + 3] = src[si + 3];
            }
            PixelFormat::Unknown => {
                dst[di] = src[si];
                dst[di + 1] = src[si + 1];
                dst[di + 2] = src[si + 2];
                dst[di + 3] = src[si + 3];
            }
        }
    }
}
