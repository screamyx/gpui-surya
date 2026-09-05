use super::*;
use crate::directx_devices::DirectXDevices;
use gpui::{Bounds, ContentMask, ExternalTexture, ScaledPixels, point, size};
use std::os::windows::io::{FromRawHandle, OwnedHandle};
use windows::Win32::Foundation::HANDLE;

fn descriptor(width: u32, height: u32, shared: bool) -> D3D11_TEXTURE2D_DESC {
    D3D11_TEXTURE2D_DESC {
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
        BindFlags: (D3D11_BIND_RENDER_TARGET | D3D11_BIND_SHADER_RESOURCE).0 as u32,
        MiscFlags: if shared {
            (D3D11_RESOURCE_MISC_SHARED_NTHANDLE | D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX).0 as u32
        } else {
            0
        },
        ..Default::default()
    }
}

fn create_texture(device: &ID3D11Device, desc: &D3D11_TEXTURE2D_DESC) -> Result<ID3D11Texture2D> {
    let mut texture = None;
    unsafe { device.CreateTexture2D(desc, None, Some(&mut texture)) }?;
    texture.context("test texture")
}

fn view(device: &ID3D11Device, texture: &ID3D11Texture2D) -> Result<ID3D11RenderTargetView> {
    let mut view = None;
    unsafe { device.CreateRenderTargetView(texture, None, Some(&mut view)) }?;
    view.context("test view")
}

fn bounds(x: f32, y: f32, width: f32, height: f32) -> Bounds<ScaledPixels> {
    Bounds::new(
        point(ScaledPixels(x), ScaledPixels(y)),
        size(ScaledPixels(width), ScaledPixels(height)),
    )
}

#[test]
fn shared_texture_draws_clipped_pixels_after_producer_owner_drops() -> Result<()> {
    let producer = DirectXDevices::new()?;
    let consumer = DirectXRendererDevices::new(&DirectXDevices::new()?, true)?;
    let source = create_texture(&producer.device, &descriptor(8, 8, true))?;
    let source_view = view(&producer.device, &source)?;
    let mutex: IDXGIKeyedMutex = source.cast()?;
    let guard = KeyedGuard::acquire(mutex)?;
    unsafe {
        producer
            .device_context
            .ClearRenderTargetView(&source_view, &[1.0, 0.0, 0.0, 1.0]);
    }
    guard.release()?;
    unsafe {
        producer.device_context.Flush();
    }
    let resource: IDXGIResource1 = source.cast()?;
    let handle = unsafe {
        resource.CreateSharedHandle(
            None,
            DXGI_SHARED_RESOURCE_READ.0 | DXGI_SHARED_RESOURCE_WRITE.0,
            None,
        )
    }?;
    let adapter = unsafe { producer.adapter.GetDesc1() }?;
    let owner = unsafe {
        ExternalTexture::new(
            OwnedHandle::from_raw_handle(handle.0),
            (adapter.AdapterLuid.LowPart, adapter.AdapterLuid.HighPart),
            (8, 8),
            ExternalTextureSync::KeyedMutex,
        )
    };
    let surface = PaintSurface {
        order: 0,
        bounds: bounds(-2.0, 3.0, 9.0, 8.0),
        content_mask: ContentMask {
            bounds: bounds(2.0, 4.0, 12.0, 3.0),
        },
        external_texture: owner.clone(),
    };
    // Keep only the scene's owner, as happens after tab close or resize.
    drop(owner);
    drop(resource);
    drop(source_view);
    drop(source);
    drop(producer);
    let target = create_texture(&consumer.device, &descriptor(16, 16, false))?;
    let target_view = Some(view(&consumer.device, &target)?);
    unsafe {
        consumer.device_context.ClearRenderTargetView(
            target_view.as_ref().context("target view")?,
            &[0.0, 0.0, 0.0, 1.0],
        );
    }
    // A new keyed share may still be completing the producer clear. This
    // bounded wait is test setup, not the renderer's nonblocking draw path.
    let device: ID3D11Device1 = consumer.device.cast()?;
    let opened: ID3D11Texture2D =
        unsafe { device.OpenSharedResource1(HANDLE(surface.external_texture.as_raw_handle())) }?;
    let mutex: IDXGIKeyedMutex = opened.cast()?;
    let result = unsafe { (mutex.vtable().AcquireSync)(mutex.as_raw(), 0, 5000) };
    ensure!(result == S_OK, "test producer completion: {result:?}");
    unsafe { mutex.ReleaseSync(0) }?;
    let mut cache = ExternalTextureCache::default();
    let mut completed = 0;
    for _ in 0..32 {
        assert!(draw_one(
            &mut cache,
            &consumer,
            &target,
            &target_view,
            &surface
        )?);
        completed += 1;
        // Complete reads before reacquiring key 0 on another opened wrapper.
        unsafe {
            consumer.device_context.Flush();
        }
    }
    assert_eq!(
        cache.len(),
        1,
        "one open for repeated draws of the same owner"
    );
    let retained = surface.external_texture.clone();
    drop(surface);
    cache.prune();
    assert_eq!(cache.len(), 1, "another scene clone keeps the cache alive");
    drop(retained);
    cache.prune();
    assert_eq!(cache.len(), 0, "cache must not retain a retired allocation");
    let staging = create_texture(
        &consumer.device,
        &D3D11_TEXTURE2D_DESC {
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            ..descriptor(16, 16, false)
        },
    )?;
    unsafe {
        consumer.device_context.OMSetRenderTargets(None, None);
        consumer.device_context.CopyResource(&staging, &target);
    }
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    unsafe {
        consumer
            .device_context
            .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
    }?;
    let mut mismatches = 0;
    for y in 0..16usize {
        for x in 0..16usize {
            let pixel = unsafe {
                std::slice::from_raw_parts(
                    (mapped.pData as *const u8).add(y * mapped.RowPitch as usize + x * 4),
                    4,
                )
            };
            let expected = if (2..6).contains(&x) && (4..7).contains(&y) {
                [0, 0, 255, 255]
            } else {
                [0, 0, 0, 255]
            };
            if pixel != expected {
                mismatches += 1;
            }
        }
    }
    unsafe {
        consumer.device_context.Unmap(&staging, 0);
    }
    assert_eq!(mismatches, 0, "GPU copy violated source/target clipping");
    println!(
        "external texture GPU test: asked=32 frames={completed} dropped=0 pixels_asked=256 pixels_matched={}",
        256 - mismatches
    );
    Ok(())
}

#[test]
fn keyed_timeout_never_counts_as_acquired() -> Result<()> {
    let devices = DirectXDevices::new()?;
    let source = create_texture(&devices.device, &descriptor(8, 8, true))?;
    let mutex: IDXGIKeyedMutex = source.cast()?;
    let result = unsafe { (mutex.vtable().AcquireSync)(mutex.as_raw(), 0, 1000) };
    ensure!(result == S_OK, "initial acquire failed");
    // Leave key 1 available: key 0 must return WAIT_TIMEOUT (a positive HRESULT).
    unsafe { mutex.ReleaseSync(1) }?;
    assert!(KeyedGuard::acquire(mutex.clone()).is_err());
    let result = unsafe { (mutex.vtable().AcquireSync)(mutex.as_raw(), 1, 1000) };
    ensure!(result == S_OK, "key 1 acquire failed");
    unsafe { mutex.ReleaseSync(0) }?;
    KeyedGuard::acquire(mutex)?.release()?;
    Ok(())
}
