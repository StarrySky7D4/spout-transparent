//! Opt-in Release measurements. Results are measurements, not speed assertions.
use serde::Serialize;
use std::hint::black_box;
use std::mem::size_of;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use windows::core::Interface;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::{Common::*, IDXGIDevice};

use crate::crop::visible_bounds;
use crate::dx::pipeline::create_pipeline;
use crate::dx::staging::AlphaReadback;
use crate::renderer::render_frame;
use crate::scheduler::{Scheduler, Task};

#[derive(Serialize)]
struct Measurement {
    name: String,
    unit: &'static str,
    samples: Vec<f64>,
    median: f64,
    p95: f64,
    maximum: f64,
}

impl Measurement {
    fn new(name: impl Into<String>, samples: Vec<f64>) -> Self {
        assert!(!samples.is_empty());
        let mut sorted = samples.clone();
        sorted.sort_by(f64::total_cmp);
        let median = sorted[sorted.len() / 2];
        let p95 = sorted[((sorted.len() as f64 * 0.95).ceil() as usize - 1).min(sorted.len() - 1)];
        let maximum = *sorted.last().unwrap();
        let name = name.into();
        println!(
            "{name}: median={median:.4} ms p95={p95:.4} ms n={}",
            samples.len()
        );
        Self {
            name,
            unit: "ms",
            samples,
            median,
            p95,
            maximum,
        }
    }
}

#[derive(Serialize)]
struct Report {
    unix_timestamp: u64,
    release_build: bool,
    gpu_adapter: String,
    live_sender_names: Vec<String>,
    live_sender_sizes: Vec<(String, u32, u32, u32)>,
    event_idle_about_to_wait: u64,
    measurements: Vec<Measurement>,
    notes: Vec<&'static str>,
}

fn cpu_measurements(output: &mut Vec<Measurement>) {
    for (width, height) in [(1920u32, 1080u32), (3840, 2160)] {
        for pattern in ["opaque", "transparent", "center_block"] {
            let mut alpha = vec![0u8; (width * height) as usize];
            if pattern == "opaque" {
                alpha.fill(255);
            }
            if pattern == "center_block" {
                for y in height * 3 / 8..height * 5 / 8 {
                    let start = (y * width + width * 3 / 8) as usize;
                    alpha[start..start + width as usize / 4].fill(255);
                }
            }
            for _ in 0..10 {
                black_box(visible_bounds(black_box(&alpha), width, height));
            }
            let samples = (0..120)
                .map(|_| {
                    let start = Instant::now();
                    black_box(visible_bounds(black_box(&alpha), width, height));
                    start.elapsed().as_secs_f64() * 1000.0
                })
                .collect();
            output.push(Measurement::new(
                format!("cpu_bounds_{width}x{height}_{pattern}"),
                samples,
            ));
        }
    }
    let mut scheduler = Scheduler::default();
    let now = Instant::now();
    let samples = (0..100)
        .map(|_| {
            let start = Instant::now();
            for i in 0..1000u64 {
                let scheduler = black_box(&mut scheduler);
                let tick = now + Duration::from_micros(i);
                scheduler.set(Task::Frame, Some(tick));
                scheduler.set(Task::Readback, Some(tick + Duration::from_millis(2)));
                scheduler.set(Task::Source, Some(tick + Duration::from_millis(100)));
                black_box(scheduler.next_wake());
                black_box(scheduler.take_due(Task::Frame, tick));
                scheduler.set(Task::Readback, None);
            }
            start.elapsed().as_secs_f64() // seconds for 1000 cycles == milliseconds/cycle
        })
        .collect();
    output.push(Measurement::new("cpu_scheduler_cycle", samples));
}

fn create_query(device: &ID3D11Device, kind: D3D11_QUERY) -> windows::core::Result<ID3D11Query> {
    let mut query = None;
    unsafe {
        device.CreateQuery(
            &D3D11_QUERY_DESC {
                Query: kind,
                MiscFlags: 0,
            },
            Some(&mut query),
        )?;
    }
    query.ok_or_else(crate::dx::missing_object)
}

/// GetData's generated Result<()> wrapper loses S_FALSE (not ready), so preserve
/// the exact HRESULT. Inaccurate readiness would produce invalid zero timings.
fn query_data<T: Default>(
    context: &ID3D11DeviceContext,
    query: &ID3D11Query,
) -> windows::core::Result<T> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let mut result = T::default();
        let hr = unsafe {
            (Interface::vtable(context).GetData)(
                Interface::as_raw(context),
                Interface::as_raw(query),
                (&mut result as *mut T).cast(),
                size_of::<T>() as u32,
                D3D11_ASYNC_GETDATA_DONOTFLUSH.0 as u32,
            )
        };
        if hr.0 == 0 {
            return Ok(result);
        }
        if hr.0 != 1 {
            return Err(windows::core::Error::from_hresult(hr));
        }
        assert!(Instant::now() < deadline, "GPU query timed out");
        std::thread::sleep(Duration::from_millis(1));
    }
}

struct Timer {
    disjoint: ID3D11Query,
    start: ID3D11Query,
    end: ID3D11Query,
}

impl Timer {
    fn new(device: &ID3D11Device) -> windows::core::Result<Self> {
        Ok(Self {
            disjoint: create_query(device, D3D11_QUERY_TIMESTAMP_DISJOINT)?,
            start: create_query(device, D3D11_QUERY_TIMESTAMP)?,
            end: create_query(device, D3D11_QUERY_TIMESTAMP)?,
        })
    }
    fn begin(&self, context: &ID3D11DeviceContext) {
        unsafe {
            context.Begin(&self.disjoint);
            context.End(&self.start);
        }
    }
    fn finish(&self, context: &ID3D11DeviceContext) -> windows::core::Result<Option<f64>> {
        unsafe {
            context.End(&self.end);
            context.End(&self.disjoint);
            context.Flush();
        }
        let disjoint: D3D11_QUERY_DATA_TIMESTAMP_DISJOINT = query_data(context, &self.disjoint)?;
        let start: u64 = query_data(context, &self.start)?;
        let end: u64 = query_data(context, &self.end)?;
        if disjoint.Disjoint.as_bool() || disjoint.Frequency == 0 || end < start {
            return Ok(None);
        }
        Ok(Some(
            (end - start) as f64 / disjoint.Frequency as f64 * 1000.0,
        ))
    }
}

fn gpu_measurements(output: &mut Vec<Measurement>) -> windows::core::Result<String> {
    let (device, context) = crate::dx::device::create_dx11_device_auto()?;
    let adapter = unsafe { device.cast::<IDXGIDevice>()?.GetAdapter()?.GetDesc()? };
    let end = adapter
        .Description
        .iter()
        .position(|c| *c == 0)
        .unwrap_or(adapter.Description.len());
    let adapter_name = String::from_utf16_lossy(&adapter.Description[..end]);
    println!("GPU adapter: {adapter_name}");
    let pipeline = create_pipeline(&device)?;
    for (width, height) in [(1920u32, 1080u32), (3840, 2160)] {
        let mut pixels = vec![0u8; (width * height * 4) as usize];
        for y in height * 3 / 8..height * 5 / 8 {
            let start = (y * width + width * 3 / 8) as usize * 4;
            pixels[start..start + width as usize].fill(255);
        }
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            ..Default::default()
        };
        let initial = D3D11_SUBRESOURCE_DATA {
            pSysMem: pixels.as_ptr().cast(),
            SysMemPitch: width * 4,
            SysMemSlicePitch: 0,
        };
        let mut texture = None;
        let mut srv = None;
        let mut target = None;
        let mut rtv = None;
        unsafe {
            device.CreateTexture2D(&desc, Some(&initial), Some(&mut texture))?;
            device.CreateShaderResourceView(texture.as_ref().unwrap(), None, Some(&mut srv))?;
            device.CreateTexture2D(
                &D3D11_TEXTURE2D_DESC {
                    BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
                    ..desc
                },
                None,
                Some(&mut target),
            )?;
            device.CreateRenderTargetView(target.as_ref().unwrap(), None, Some(&mut rtv))?;
        }
        let srv = srv.unwrap();
        let rtv = rtv.unwrap();
        let timer = Timer::new(&device)?;
        let mut draw_gpu = [Vec::new(), Vec::new()];
        let mut draw_cpu = [Vec::new(), Vec::new()];
        // Alternate A/B order to reduce thermal, boost and other-GPU-load bias.
        for i in 0..72 {
            for mode in if i % 2 == 0 { [0, 1] } else { [1, 0] } {
                timer.begin(&context);
                let cpu_start = Instant::now();
                if mode == 1 {
                    unsafe {
                        context.ClearRenderTargetView(&rtv, &[0.0; 4]);
                    }
                }
                render_frame(&context, &pipeline, &srv, &rtv, width, height);
                let cpu_ms = cpu_start.elapsed().as_secs_f64() * 1000.0;
                if let Some(ms) = timer.finish(&context)? {
                    if i >= 12 {
                        draw_gpu[mode].push(ms);
                        draw_cpu[mode].push(cpu_ms);
                    }
                }
            }
        }
        for mode in 0..2 {
            let label = if mode == 1 {
                "with_clear"
            } else {
                "without_clear"
            };
            output.push(Measurement::new(
                format!("gpu_draw_{width}x{height}_{label}"),
                std::mem::take(&mut draw_gpu[mode]),
            ));
            output.push(Measurement::new(
                format!("cpu_submit_{width}x{height}_{label}"),
                std::mem::take(&mut draw_cpu[mode]),
            ));
        }
        let mut readback = AlphaReadback::new(&device, width, height)?;
        let mut alpha = Vec::new();
        let mut gpu = Vec::new();
        let mut copy = Vec::new();
        let mut ready_wait = Vec::new();
        for i in 0..72u64 {
            timer.begin(&context);
            assert!(readback.enqueue_extract(&context, &pipeline, &srv, i));
            let measured = timer.finish(&context)?;
            let ready_start = Instant::now();
            let deadline = ready_start + Duration::from_secs(3);
            let cpu_ms = loop {
                let attempt = Instant::now();
                let epoch = readback.try_read(&context, &mut alpha)?;
                let duration = attempt.elapsed().as_secs_f64() * 1000.0;
                if let Some(epoch) = epoch {
                    assert_eq!(epoch, i);
                    break duration;
                }
                assert!(Instant::now() < deadline, "staging readback timed out");
                std::thread::sleep(Duration::from_millis(1));
            };
            let wall_ms = ready_start.elapsed().as_secs_f64() * 1000.0;
            assert_eq!(alpha.len(), (width * height) as usize);
            if i >= 12 {
                if let Some(ms) = measured {
                    gpu.push(ms);
                    copy.push(cpu_ms);
                    ready_wait.push(wall_ms - cpu_ms);
                }
            }
        }
        assert!(visible_bounds(&alpha, width, height).is_some());
        output.push(Measurement::new(
            format!("gpu_alpha_extract_copy_{width}x{height}"),
            gpu,
        ));
        output.push(Measurement::new(
            format!("cpu_map_copy_ready_{width}x{height}"),
            copy,
        ));
        output.push(Measurement::new(
            format!("readback_wait_after_timestamp_{width}x{height}"),
            ready_wait,
        ));
    }
    unsafe {
        context.ClearState();
        context.Flush();
    }
    Ok(adapter_name)
}

fn event_measurements(output: &mut Vec<Measurement>) -> u64 {
    use std::cell::RefCell;
    use std::rc::Rc;
    use winit::event::{Event, StartCause};
    use winit::event_loop::{ControlFlow, EventLoop};
    use winit::platform::windows::EventLoopBuilderExtWindows;
    #[derive(Debug)]
    enum TimedEvent {
        Input(usize, Instant),
        Done,
    }
    let event_loop = EventLoop::<TimedEvent>::with_user_event()
        .with_any_thread(true)
        .build()
        .unwrap();
    let proxy = event_loop.create_proxy();
    let worker = std::thread::spawn(move || {
        for i in 0..600 {
            proxy
                .send_event(TimedEvent::Input(i, Instant::now()))
                .unwrap();
            std::thread::sleep(Duration::from_millis(2));
        }
        proxy.send_event(TimedEvent::Done).unwrap();
    });
    let samples = Rc::new(RefCell::new(Vec::new()));
    let received = Rc::clone(&samples);
    let idle_wakes = Rc::new(RefCell::new(0u64));
    let wakes = Rc::clone(&idle_wakes);
    let mut idle_deadline = None;
    let mut sequence = 0;
    let timeout = Instant::now() + Duration::from_secs(20);
    #[allow(deprecated)]
    event_loop
        .run(move |event, elwt| match event {
            Event::UserEvent(TimedEvent::Input(i, sent)) => {
                assert_eq!(i, sequence, "event sequence changed");
                sequence += 1;
                if i >= 100 {
                    received
                        .borrow_mut()
                        .push(sent.elapsed().as_secs_f64() * 1000.0);
                }
            }
            Event::UserEvent(TimedEvent::Done) => {
                idle_deadline = Some(Instant::now() + Duration::from_secs(2));
            }
            Event::AboutToWait => {
                if idle_deadline.is_some() {
                    *wakes.borrow_mut() += 1;
                }
                elwt.set_control_flow(ControlFlow::WaitUntil(idle_deadline.unwrap_or(timeout)));
            }
            Event::NewEvents(StartCause::ResumeTimeReached { .. }) => elwt.exit(),
            _ => {}
        })
        .unwrap();
    worker.join().unwrap();
    assert_eq!(samples.borrow().len(), 500);
    output.push(Measurement::new(
        "event_proxy_delivery",
        samples.borrow().clone(),
    ));
    let result = *idle_wakes.borrow();
    println!("Event-loop idle AboutToWait count during 2-second tail: {result}");
    result
}

pub(crate) fn measure_performance() -> Result<(), Box<dyn std::error::Error>> {
    if cfg!(debug_assertions) {
        return Err("Use the optimized cargo bench profile".into());
    }
    let live_senders = crate::spout::discovery::discover_sender_names()?;
    let live_sender_sizes = live_senders
        .iter()
        .filter_map(|name| {
            crate::spout::read_sender_info(name, 0)
                .ok()
                .map(|info| (name.display_name(), info.width, info.height, info.format))
        })
        .collect::<Vec<_>>();
    println!("Live sender metadata: {live_sender_sizes:?}");
    let live_sender_names = live_senders
        .iter()
        .map(|name| name.display_name())
        .collect::<Vec<_>>();
    println!("Live Spout senders: {}", live_sender_names.len());
    let mut measurements = Vec::new();
    cpu_measurements(&mut measurements);
    let gpu_adapter = gpu_measurements(&mut measurements)?;
    let idle_wakes = event_measurements(&mut measurements);
    let report = Report { unix_timestamp: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
        release_build: true, gpu_adapter, live_sender_names, live_sender_sizes,
        event_idle_about_to_wait: idle_wakes, measurements,
        notes: vec!["Synthetic source texture; no live sender registration or user window changes",
            "GPU timings use timestamp/disjoint queries; disjoint samples are rejected",
            "Ready Map timing excludes GPU waiting; query polling and Flush are measurement overhead",
            "No Present/DWM visible-frame FPS or complete mouse-forwarding latency measured",
            "Static synthetic textures and isolated serial samples can differ from streaming workloads"] };
    let dir = std::env::var_os("SPOUT_PERF_OUTPUT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("target/performance"));
    std::fs::create_dir_all(&dir)?;
    std::fs::write(
        dir.join("results.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    let mut csv = String::from("name,unit,count,median,p95,maximum\n");
    for measurement in &report.measurements {
        csv.push_str(&format!(
            "{},{},{},{:.8},{:.8},{:.8}\n",
            measurement.name,
            measurement.unit,
            measurement.samples.len(),
            measurement.median,
            measurement.p95,
            measurement.maximum
        ));
    }
    std::fs::write(dir.join("summary.csv"), csv)?;
    println!("Performance results saved to {}", dir.display());
    Ok(())
}
