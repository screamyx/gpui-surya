#[cfg(not(windows))]
fn main() {
    eprintln!("This probe requires Windows D3D11.");
}

#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    probe::run()
}

#[cfg(windows)]
mod probe {
    use anyhow::{Context, Result};
    use gpui::{
        App, Bounds, Context as GpuiContext, ExternalTexture, ExternalTextureSync, Window,
        WindowBounds, WindowOptions, canvas, div, point, prelude::*, px, rgb, size,
    };
    use std::{
        os::windows::io::{FromRawHandle, OwnedHandle},
        sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        },
        time::Duration,
    };
    use windows::{
        Win32::{
            Foundation::HMODULE,
            Graphics::{
                Direct3D::*,
                Direct3D11::*,
                Dxgi::{Common::*, *},
            },
        },
        core::Interface,
    };

    struct Probe {
        texture: ExternalTexture,
        asked: Arc<AtomicU64>,
    }

    impl Render for Probe {
        fn render(&mut self, _: &mut Window, _: &mut GpuiContext<Self>) -> impl IntoElement {
            let texture = self.texture.clone();
            let asked = self.asked.clone();
            div().size_full().bg(rgb(0x17202a)).child(
                canvas(
                    |_, _, _| (),
                    move |_, _, window, _| {
                        let scale = window.scale_factor();
                        let bounds = Bounds::new(
                            point(px(48.0 / scale), px(80.0 / scale)),
                            size(px(320.0 / scale), px(200.0 / scale)),
                        );
                        window.paint_external_texture(bounds, &texture);
                        let frame = asked.fetch_add(1, Ordering::Relaxed) + 1;
                        if frame < 60 {
                            window.request_animation_frame();
                        }
                        if frame == 60 {
                            println!("probe: asked=60 submitted={frame}");
                        }
                    },
                )
                .size_full(),
            )
        }
    }

    fn texture() -> Result<ExternalTexture> {
        let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }?;
        let adapter = unsafe { factory.EnumAdapters1(0) }?;
        let adapter_desc = unsafe { adapter.GetDesc1() }?;
        let mut device = None;
        let mut context = None;
        unsafe {
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
        }?;
        let device = device.context("D3D11 device")?;
        let context = context.context("D3D11 context")?;
        let desc = D3D11_TEXTURE2D_DESC {
            Width: 320,
            Height: 200,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_RENDER_TARGET | D3D11_BIND_SHADER_RESOURCE).0 as u32,
            MiscFlags: (D3D11_RESOURCE_MISC_SHARED_NTHANDLE | D3D11_RESOURCE_MISC_SHARED).0 as u32,
            ..Default::default()
        };
        let mut texture = None;
        unsafe { device.CreateTexture2D(&desc, None, Some(&mut texture)) }?;
        let texture = texture.context("shared texture")?;
        let mut view = None;
        unsafe { device.CreateRenderTargetView(&texture, None, Some(&mut view)) }?;
        let view = view.context("texture view")?;
        unsafe {
            context.ClearRenderTargetView(&view, &[0.12, 0.74, 0.43, 1.0]);
        }
        // Observe completion before publishing the immutable client texture.
        let mut query = None;
        unsafe {
            device.CreateQuery(
                &D3D11_QUERY_DESC {
                    Query: D3D11_QUERY_EVENT,
                    MiscFlags: 0,
                },
                Some(&mut query),
            )
        }?;
        let query = query.context("completion query")?;
        unsafe {
            context.End(&query);
            context.Flush();
        }
        let started = std::time::Instant::now();
        loop {
            let mut complete = 0u32;
            unsafe { context.GetData(&query, Some((&mut complete as *mut u32).cast()), 4, 0) }?;
            if complete != 0 {
                break;
            }
            anyhow::ensure!(
                started.elapsed() < Duration::from_secs(5),
                "GPU completion timed out"
            );
            std::thread::yield_now();
        }
        let resource: IDXGIResource1 = texture.cast()?;
        let handle = unsafe {
            resource.CreateSharedHandle(
                None,
                DXGI_SHARED_RESOURCE_READ.0 | DXGI_SHARED_RESOURCE_WRITE.0,
                None,
            )
        }?;
        let handle = unsafe { OwnedHandle::from_raw_handle(handle.0) };
        println!(
            "probe texture: asked=1 created=1 width=320 height=200 adapter={}:{}",
            adapter_desc.AdapterLuid.HighPart, adapter_desc.AdapterLuid.LowPart
        );
        // This freshly allocated texture will never be written again; GPU clear
        // completed above and the NT handle keeps its allocation alive.
        Ok(unsafe {
            ExternalTexture::new(
                handle,
                (
                    adapter_desc.AdapterLuid.LowPart,
                    adapter_desc.AdapterLuid.HighPart,
                ),
                (320, 200),
                ExternalTextureSync::ProducerComplete,
            )
        })
    }

    pub fn run() -> Result<()> {
        let texture = texture()?;
        gpui_platform::application().run(move |cx: &mut App| {
            let result = cx.open_window(
                WindowOptions {
                    titlebar: Some(gpui::TitlebarOptions {
                        title: Some("External texture proof".into()),
                        ..Default::default()
                    }),
                    window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                        None,
                        size(px(480.0), px(360.0)),
                        cx,
                    ))),
                    ..Default::default()
                },
                |_, cx| {
                    cx.new(|_| Probe {
                        texture,
                        asked: Arc::new(AtomicU64::new(0)),
                    })
                },
            );
            if let Err(error) = result {
                eprintln!("probe window: asked=1 opened=0 error={error:#}");
                cx.quit();
                return;
            }
            cx.activate(true);
        });
        Ok(())
    }
}
