use windows::core::PCSTR;
use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D11::*;

const VS_SRC: &str = r#"
struct VSInput { uint id : SV_VertexID; };
struct VSOutput { float4 pos : SV_POSITION; float2 uv : TEXCOORD0; };
VSOutput main(VSInput input) {
    VSOutput output;
    float2 pos[3] = { float2(-1.0, -3.0), float2(3.0, 1.0), float2(-1.0, 1.0) };
    output.pos = float4(pos[input.id], 0.0, 1.0);
    output.uv = output.pos.xy * 0.5 + 0.5;
    output.uv.y = 1.0 - output.uv.y;
    return output;
}
"#;

const PS_SRC: &str = r#"
Texture2D tex : register(t0);
SamplerState sam : register(s0);
float4 main(float4 pos : SV_POSITION, float2 uv : TEXCOORD0) : SV_TARGET {
    float4 color = tex.Sample(sam, uv);
    return float4(color.rgb * color.a, color.a);
}
"#;

const D3DCOMPILE_DEBUG: u32 = 0x1;
const D3DCOMPILE_SKIP_OPTIMIZATION: u32 = 0x2000;

fn compile_flags() -> u32 {
    if cfg!(debug_assertions) {
        D3DCOMPILE_DEBUG | D3DCOMPILE_SKIP_OPTIMIZATION
    } else {
        0
    }
}

pub struct Pipeline {
    pub vs: ID3D11VertexShader,
    pub ps: ID3D11PixelShader,
    pub sampler: ID3D11SamplerState,
    pub raster_state: ID3D11RasterizerState,
}

fn compile_shader(code: &str, target: &str) -> windows::core::Result<Vec<u8>> {
    let c_target =
        std::ffi::CString::new(target).map_err(|_| windows::core::Error::from_win32())?;
    let c_entry = std::ffi::CString::new("main").map_err(|_| windows::core::Error::from_win32())?;
    let mut blob = None;
    let mut error_blob = None;
    let hr = unsafe {
        D3DCompile(
            code.as_ptr() as *const _,
            code.len(),
            PCSTR::null(),
            None::<*const _>,
            None,
            PCSTR(c_entry.as_ptr() as *const u8),
            PCSTR(c_target.as_ptr() as *const u8),
            compile_flags(),
            0,
            &mut blob,
            Some(&mut error_blob),
        )
    };
    if let Some(ref err) = error_blob {
        let err_ptr = unsafe { err.GetBufferPointer() as *const u8 };
        let err_size = unsafe { err.GetBufferSize() };
        let err_msg = unsafe { std::slice::from_raw_parts(err_ptr, err_size) };
        let msg = String::from_utf8_lossy(err_msg);
        log::error!("Shader compile failed (target={target}): {msg}");
    }
    hr?;
    let blob = blob.ok_or_else(windows::core::Error::from_win32)?;
    let buffer_ptr = unsafe { blob.GetBufferPointer() as *const u8 };
    let buffer_size = unsafe { blob.GetBufferSize() };
    Ok(unsafe { std::slice::from_raw_parts(buffer_ptr, buffer_size) }.to_vec())
}

pub fn create_pipeline(device: &ID3D11Device) -> windows::core::Result<Pipeline> {
    let vs_blob = compile_shader(VS_SRC, "vs_5_0")?;
    let ps_blob = compile_shader(PS_SRC, "ps_5_0")?;

    let mut vs = None;
    unsafe {
        device.CreateVertexShader(&vs_blob, None, Some(&mut vs))?;
    }
    let mut ps = None;
    unsafe {
        device.CreatePixelShader(&ps_blob, None, Some(&mut ps))?;
    }

    let sampler_desc = D3D11_SAMPLER_DESC {
        Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
        AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
        AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
        AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
        ComparisonFunc: D3D11_COMPARISON_NEVER,
        ..Default::default()
    };
    let mut sampler = None;
    unsafe {
        device.CreateSamplerState(&sampler_desc, Some(&mut sampler))?;
    }

    let raster_desc = D3D11_RASTERIZER_DESC {
        FillMode: D3D11_FILL_SOLID,
        CullMode: D3D11_CULL_NONE,
        FrontCounterClockwise: false.into(),
        DepthBias: 0,
        DepthBiasClamp: 0.0,
        SlopeScaledDepthBias: 0.0,
        DepthClipEnable: true.into(),
        ScissorEnable: false.into(),
        MultisampleEnable: false.into(),
        AntialiasedLineEnable: false.into(),
    };
    let mut raster_state = None;
    unsafe {
        device.CreateRasterizerState(&raster_desc, Some(&mut raster_state))?;
    }

    Ok(Pipeline {
        vs: vs.ok_or_else(crate::dx::missing_object)?,
        ps: ps.ok_or_else(crate::dx::missing_object)?,
        sampler: sampler.ok_or_else(crate::dx::missing_object)?,
        raster_state: raster_state.ok_or_else(crate::dx::missing_object)?,
    })
}
