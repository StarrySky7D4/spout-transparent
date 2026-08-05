use std::collections::VecDeque;

use windows::core::Interface;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Dxgi::DXGI_ERROR_WAS_STILL_DRAWING;

const READBACK_BUFFER_COUNT: usize = 3;

struct ReadbackSlot {
    texture: ID3D11Texture2D,
    resource: ID3D11Resource,
}

pub struct AlphaReadback {
    slots: Vec<ReadbackSlot>,
    pending: VecDeque<usize>,
    next_write: usize,
    width: u32,
    height: u32,
}

impl AlphaReadback {
    pub fn new(device: &ID3D11Device, width: u32, height: u32) -> windows::core::Result<Self> {
        if width == 0 || height == 0 {
            return Err(crate::dx::invalid_argument());
        }

        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
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

        let mut slots = Vec::with_capacity(READBACK_BUFFER_COUNT);
        for _ in 0..READBACK_BUFFER_COUNT {
            let mut texture = None;
            unsafe { device.CreateTexture2D(&desc, None, Some(&mut texture))? };
            let texture = texture.ok_or_else(crate::dx::missing_object)?;
            let resource = texture.cast()?;
            slots.push(ReadbackSlot { texture, resource });
        }

        Ok(Self {
            slots,
            pending: VecDeque::with_capacity(READBACK_BUFFER_COUNT),
            next_write: 0,
            width,
            height,
        })
    }

    pub fn enqueue_copy(
        &mut self,
        context: &ID3D11DeviceContext,
        source: &ID3D11Texture2D,
    ) -> windows::core::Result<bool> {
        if self.pending.len() == self.slots.len() {
            return Ok(false);
        }

        let source: ID3D11Resource = source.cast()?;
        let index = self.next_write;
        unsafe {
            context.CopyResource(&self.slots[index].resource, &source);
        }
        self.pending.push_back(index);
        self.next_write = (index + 1) % self.slots.len();
        Ok(true)
    }

    pub fn try_read(
        &mut self,
        context: &ID3D11DeviceContext,
        alpha: &mut Vec<u8>,
    ) -> windows::core::Result<bool> {
        let Some(&index) = self.pending.front() else {
            return Ok(false);
        };

        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        let map_result = unsafe {
            context.Map(
                &self.slots[index].texture,
                0,
                D3D11_MAP_READ,
                D3D11_MAP_FLAG_DO_NOT_WAIT.0 as u32,
                Some(&mut mapped),
            )
        };
        if let Err(error) = map_result {
            if error.code() == DXGI_ERROR_WAS_STILL_DRAWING {
                return Ok(false);
            }
            self.pending.pop_front();
            return Err(error);
        }

        alpha.clear();
        let Some(pixel_count) = (self.width as usize).checked_mul(self.height as usize) else {
            unsafe { context.Unmap(&self.slots[index].texture, 0) };
            self.pending.pop_front();
            return Err(crate::dx::invalid_argument());
        };
        alpha.resize(pixel_count, 0);

        let pitch = mapped.RowPitch as usize;
        let source = mapped.pData as *const u8;
        let row_width = self.width as usize;
        for (y, output_row) in alpha.chunks_exact_mut(row_width).enumerate() {
            let source_row = unsafe { source.add(y * pitch) };
            for (x, output_alpha) in output_row.iter_mut().enumerate() {
                *output_alpha = unsafe { *source_row.add(x * 4 + 3) };
            }
        }

        unsafe { context.Unmap(&self.slots[index].texture, 0) };
        self.pending.pop_front();
        Ok(true)
    }
}
