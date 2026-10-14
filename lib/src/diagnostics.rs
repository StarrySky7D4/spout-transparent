use windows::core::Interface;
use windows::Win32::Graphics::Direct3D11::*;

#[cfg(debug_assertions)]
pub(crate) fn debug_capture_enabled() -> bool {
    std::env::var("SPOUT_DEBUG_CAPTURE")
        .map(|value| matches!(value.to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

#[cfg(debug_assertions)]
pub(crate) fn save_texture_to_bmp(
    device: &ID3D11Device,
    ctx: &ID3D11DeviceContext,
    tex: &ID3D11Texture2D,
    label: &str,
    path: &std::path::Path,
) {
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    unsafe {
        tex.GetDesc(&mut desc);
    }
    let cap_desc = D3D11_TEXTURE2D_DESC {
        Width: desc.Width,
        Height: desc.Height,
        Format: desc.Format,
        MipLevels: 1,
        ArraySize: 1,
        SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
    };
    let mut cap = None;
    if unsafe { device.CreateTexture2D(&cap_desc, None, Some(&mut cap)) }.is_err() {
        log::error!("save_texture_to_bmp: CreateTexture2D staging failed");
        return;
    }
    let cap = match cap {
        Some(c) => c,
        None => return,
    };
    let src = match tex.cast::<ID3D11Resource>() {
        Ok(r) => r,
        Err(e) => {
            log::error!("cast src failed: {e:?}");
            return;
        }
    };
    let dst = match cap.cast::<ID3D11Resource>() {
        Ok(r) => r,
        Err(e) => {
            log::error!("cast dst failed: {e:?}");
            return;
        }
    };
    unsafe {
        ctx.CopyResource(&dst, &src);
    }
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    if unsafe { ctx.Map(&cap, 0, D3D11_MAP_READ, 0, Some(&mut mapped)) }.is_err() {
        log::error!("save_texture_to_bmp: Map failed");
        return;
    }
    let w = desc.Width as usize;
    let h = desc.Height as usize;
    let pitch = mapped.RowPitch as usize;
    let ptr = mapped.pData as *const u8;
    let bpp = 4usize;
    let row_bytes = w * bpp;
    let bmp_row = (row_bytes + 3) & !3;
    let img_size = bmp_row * h;
    let file_size = 54 + img_size;
    let mut bmp = Vec::with_capacity(file_size);
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&(file_size as u32).to_le_bytes());
    bmp.extend_from_slice(&[0u8; 4]);
    bmp.extend_from_slice(&(54u32).to_le_bytes());
    bmp.extend_from_slice(&(40u32).to_le_bytes());
    bmp.extend_from_slice(&(w as u32).to_le_bytes());
    bmp.extend_from_slice(&(h as u32).to_le_bytes());
    bmp.extend_from_slice(&[1u8, 0, 32, 0]);
    bmp.extend_from_slice(&[0u8; 4]);
    bmp.extend_from_slice(&(img_size as u32).to_le_bytes());
    bmp.extend_from_slice(&[0x13u8, 0x0B, 0, 0, 0x13u8, 0x0B, 0, 0]);
    bmp.extend_from_slice(&[0u8; 8]);
    for y in (0..h).rev() {
        let row_ptr = unsafe { ptr.add(y * pitch) };
        let row = unsafe { std::slice::from_raw_parts(row_ptr, row_bytes) };
        bmp.extend_from_slice(row);
        let pad = bmp_row - row_bytes;
        bmp.extend(std::iter::repeat_n(0, pad));
    }
    unsafe {
        ctx.Unmap(&cap, 0);
    }
    match std::fs::write(path, &bmp) {
        Ok(_) => log::info!(
            "{label} saved: {path:?} ({w}x{h}, Format={})",
            desc.Format.0
        ),
        Err(e) => log::error!("Write {label} failed: {e}"),
    }
}

#[cfg(debug_assertions)]
pub(crate) fn sample_pixel(
    device: &ID3D11Device,
    ctx: &ID3D11DeviceContext,
    tex: &ID3D11Texture2D,
    x: u32,
    y: u32,
    label: &str,
) {
    let cap_desc = D3D11_TEXTURE2D_DESC {
        Width: 1,
        Height: 1,
        Format: unsafe {
            let mut d = D3D11_TEXTURE2D_DESC::default();
            tex.GetDesc(&mut d);
            d.Format
        },
        MipLevels: 1,
        ArraySize: 1,
        SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
    };
    let mut cap = None;
    if unsafe { device.CreateTexture2D(&cap_desc, None, Some(&mut cap)) }.is_err() {
        return;
    }
    if let Some(cap) = cap {
        let src = tex.cast::<ID3D11Resource>().ok();
        let dst = cap.cast::<ID3D11Resource>().ok();
        if let (Some(s), Some(d)) = (src, dst) {
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            unsafe {
                tex.GetDesc(&mut desc);
            }
            let box0 = D3D11_BOX {
                left: x,
                top: y,
                front: 0,
                right: x + 1,
                bottom: y + 1,
                back: 1,
            };
            unsafe {
                ctx.CopySubresourceRegion(&d, 0, 0, 0, 0, &s, 0, Some(&box0));
            }
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            if unsafe { ctx.Map(&cap, 0, D3D11_MAP_READ, 0, Some(&mut mapped)) }.is_ok() {
                let ptr = mapped.pData as *const u8;
                let b = unsafe { *ptr };
                let g = unsafe { *ptr.offset(1) };
                let r = unsafe { *ptr.offset(2) };
                let a = unsafe { *ptr.offset(3) };
                log::info!("{label} pixel({x},{y}) BGRA=({b}, {g}, {r}, {a})");
                unsafe {
                    ctx.Unmap(&cap, 0);
                }
            }
        }
    }
}

#[cfg(debug_assertions)]
pub(crate) fn capture_sender_texture(
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    sender: &crate::renderer::SenderResources,
) {
    if !debug_capture_enabled() {
        return;
    }
    let Some(exe_dir) = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(std::path::Path::to_path_buf))
    else {
        log::warn!("Could not locate the executable directory for sender capture");
        return;
    };
    save_texture_to_bmp(
        device,
        context,
        &sender.tex,
        "SenderTexture",
        &exe_dir.join("debug_01_sender_texture.bmp"),
    );
    sample_pixel(
        device,
        context,
        &sender.tex,
        sender.width / 2,
        sender.height / 2,
        "SenderCenter",
    );
}
