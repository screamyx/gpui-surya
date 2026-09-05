use std::sync::{
    OnceLock,
    atomic::{AtomicU64, Ordering},
};

use anyhow::{Context, Result, ensure};
use gpui::{ExternalTextureSync, PaintSurface};
use windows::{
    Win32::{
        Foundation::{HANDLE, S_OK},
        Graphics::{
            Direct3D11::*,
            Dxgi::{Common::*, *},
        },
    },
    core::Interface,
};

use crate::directx_renderer::DirectXRendererDevices;

static ASKED: AtomicU64 = AtomicU64::new(0);
static PAINTED: AtomicU64 = AtomicU64::new(0);
static CLIPPED: AtomicU64 = AtomicU64::new(0);
static DROPPED: AtomicU64 = AtomicU64::new(0);

pub(crate) fn draw(
    devices: &DirectXRendererDevices,
    target: &ID3D11Texture2D,
    target_view: &Option<ID3D11RenderTargetView>,
    surfaces: &[PaintSurface],
) {
    static TRACE: OnceLock<bool> = OnceLock::new();
    let trace = *TRACE.get_or_init(|| std::env::var_os("GPUI_EXTERNAL_TEXTURE_STATS").is_some());
    for surface in surfaces {
        let asked = ASKED.fetch_add(1, Ordering::Relaxed) + 1;
        let started = std::time::Instant::now();
        match draw_one(devices, target, target_view, surface) {
            Ok(true) => {
                PAINTED.fetch_add(1, Ordering::Relaxed);
            }
            Ok(false) => {
                CLIPPED.fetch_add(1, Ordering::Relaxed);
            }
            Err(error) => {
                let dropped = DROPPED.fetch_add(1, Ordering::Relaxed) + 1;
                if dropped <= 3 || dropped % 300 == 0 {
                    log::error!(
                        "external texture: asked={asked} dropped={dropped} error={error:#}"
                    );
                    if trace {
                        eprintln!(
                            "external texture: asked={asked} dropped={dropped} error={error:#}"
                        );
                    }
                }
            }
        }
        if trace {
            eprintln!(
                "external texture: asked={asked} frames={} clipped={} dropped={} submit_us={}",
                PAINTED.load(Ordering::Relaxed),
                CLIPPED.load(Ordering::Relaxed),
                DROPPED.load(Ordering::Relaxed),
                started.elapsed().as_micros(),
            );
        }
    }
}

fn draw_one(
    devices: &DirectXRendererDevices,
    target: &ID3D11Texture2D,
    target_view: &Option<ID3D11RenderTargetView>,
    surface: &PaintSurface,
) -> Result<bool> {
    let owner = &surface.external_texture;
    let adapter = unsafe { devices.adapter.GetDesc1() }?;
    ensure!(
        owner.adapter_luid() == (adapter.AdapterLuid.LowPart, adapter.AdapterLuid.HighPart),
        "external texture adapter LUID differs from renderer"
    );
    let device: ID3D11Device1 = devices.device.cast()?;
    // The scene owns the NT handle through this open and the submitted copy.
    let texture =
        unsafe { device.OpenSharedResource1::<ID3D11Texture2D>(HANDLE(owner.as_raw_handle())) }
            .context("open owned NT texture")?;
    let mut source = D3D11_TEXTURE2D_DESC::default();
    let mut destination = D3D11_TEXTURE2D_DESC::default();
    unsafe {
        texture.GetDesc(&mut source);
        target.GetDesc(&mut destination);
    }
    validate_descriptor(&source, owner.size())?;
    ensure!(
        destination.Format == source.Format && destination.SampleDesc.Count == 1,
        "external texture requires a matching BGRA8 single-sample render target"
    );
    ensure!(
        surface.bounds.size.width.0 == source.Width as f32
            && surface.bounds.size.height.0 == source.Height as f32,
        "external texture bounds must match device-pixel dimensions (scaling unsupported)"
    );
    let bounds = surface.bounds;
    let mask = surface.content_mask.bounds;
    let Some(region) = copy_region(
        [
            bounds.origin.x.0,
            bounds.origin.y.0,
            bounds.size.width.0,
            bounds.size.height.0,
        ],
        [
            mask.origin.x.0,
            mask.origin.y.0,
            mask.size.width.0,
            mask.size.height.0,
        ],
        (destination.Width, destination.Height),
    ) else {
        return Ok(false);
    };
    // Acquire after validation/clipping so no early return can strand ownership.
    let guard = match owner.synchronization() {
        ExternalTextureSync::ProducerComplete => {
            ensure!(
                source.MiscFlags & D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX.0 as u32 == 0,
                "keyed texture requires KeyedMutex synchronization"
            );
            None
        }
        ExternalTextureSync::KeyedMutex => Some(KeyedGuard::acquire(texture.cast()?)?),
    };
    unsafe {
        // CopySubresourceRegion cannot target a texture still bound for drawing.
        devices.device_context.OMSetRenderTargets(None, None);
        devices.device_context.CopySubresourceRegion(
            target,
            0,
            region.x,
            region.y,
            0,
            &texture,
            0,
            Some(&region.source),
        );
        devices
            .device_context
            .OMSetRenderTargets(Some(std::slice::from_ref(target_view)), None);
    }
    if let Some(guard) = guard {
        guard.release()?;
    }
    Ok(true)
}

fn validate_descriptor(desc: &D3D11_TEXTURE2D_DESC, size: (u32, u32)) -> Result<()> {
    ensure!(
        size.0 > 0 && size.1 > 0 && (desc.Width, desc.Height) == size,
        "external texture size differs from declared dimensions"
    );
    ensure!(
        desc.Format == DXGI_FORMAT_B8G8R8A8_UNORM
            && desc.MipLevels == 1
            && desc.ArraySize == 1
            && desc.SampleDesc.Count == 1
            && desc.SampleDesc.Quality == 0,
        "external texture must be single-mip, single-layer, single-sample BGRA8 UNORM"
    );
    Ok(())
}

struct KeyedGuard(Option<IDXGIKeyedMutex>);

impl KeyedGuard {
    fn acquire(mutex: IDXGIKeyedMutex) -> Result<Self> {
        // windows-rs Result<()> treats positive WAIT_TIMEOUT/WAIT_ABANDONED as
        // success. The raw HRESULT must be exactly S_OK before reading pixels.
        let result = unsafe { (mutex.vtable().AcquireSync)(mutex.as_raw(), 0, 0) };
        ensure!(
            result == S_OK,
            "external texture AcquireSync failed: {result:?}"
        );
        Ok(Self(Some(mutex)))
    }

    fn release(mut self) -> Result<()> {
        if let Some(mutex) = self.0.take() {
            unsafe { mutex.ReleaseSync(0) }.context("release external texture mutex")?;
        }
        Ok(())
    }
}

impl Drop for KeyedGuard {
    fn drop(&mut self) {
        if let Some(mutex) = self.0.take()
            && let Err(error) = unsafe { mutex.ReleaseSync(0) }
        {
            log::error!("release external texture mutex during unwind: {error}");
        }
    }
}

#[derive(Debug)]
struct CopyRegion {
    x: u32,
    y: u32,
    source: D3D11_BOX,
}

fn copy_region(bounds: [f32; 4], mask: [f32; 4], target: (u32, u32)) -> Option<CopyRegion> {
    if !bounds.iter().chain(mask.iter()).all(|v| v.is_finite()) {
        return None;
    }
    let [x, y, width, height] = bounds;
    let [mask_x, mask_y, mask_width, mask_height] = mask;
    let left = x.max(mask_x).max(0.0).ceil();
    let top = y.max(mask_y).max(0.0).ceil();
    let right = (x + width)
        .min(mask_x + mask_width)
        .min(target.0 as f32)
        .floor();
    let bottom = (y + height)
        .min(mask_y + mask_height)
        .min(target.1 as f32)
        .floor();
    if right <= left || bottom <= top {
        return None;
    }
    Some(CopyRegion {
        x: left as u32,
        y: top as u32,
        source: D3D11_BOX {
            left: (left - x) as u32,
            top: (top - y) as u32,
            front: 0,
            right: (right - x) as u32,
            bottom: (bottom - y) as u32,
            back: 1,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negative_origin_advances_source_when_clipped_to_target() {
        let region = copy_region(
            [-20.0, -10.0, 100.0, 80.0],
            [-100.0, -100.0, 500.0, 500.0],
            (60, 40),
        )
        .expect("visible");
        assert_eq!((region.x, region.y), (0, 0));
        assert_eq!(
            (
                region.source.left,
                region.source.top,
                region.source.right,
                region.source.bottom
            ),
            (20, 10, 80, 50)
        );
    }

    #[test]
    fn content_mask_and_empty_regions() {
        let region = copy_region(
            [20.0, 30.0, 100.0, 80.0],
            [35.0, 40.0, 20.0, 15.0],
            (200, 200),
        )
        .expect("visible");
        assert_eq!(
            (region.x, region.y, region.source.left, region.source.top),
            (35, 40, 15, 10)
        );
        assert_eq!((region.source.right, region.source.bottom), (35, 25));
        assert!(
            copy_region(
                [300.0, 0.0, 10.0, 10.0],
                [0.0, 0.0, 200.0, 200.0],
                (200, 200)
            )
            .is_none()
        );
        assert!(
            copy_region(
                [f32::NAN, 0.0, 10.0, 10.0],
                [0.0, 0.0, 200.0, 200.0],
                (200, 200)
            )
            .is_none()
        );
    }

    #[test]
    fn reject_incompatible_texture_descriptors() {
        let mut desc = D3D11_TEXTURE2D_DESC {
            Width: 64,
            Height: 32,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            ..Default::default()
        };
        assert!(validate_descriptor(&desc, (64, 32)).is_ok());
        assert!(validate_descriptor(&desc, (63, 32)).is_err());
        desc.Format = DXGI_FORMAT_R8G8B8A8_UNORM;
        assert!(validate_descriptor(&desc, (64, 32)).is_err());
        desc.Format = DXGI_FORMAT_B8G8R8A8_UNORM;
        desc.SampleDesc.Count = 4;
        assert!(validate_descriptor(&desc, (64, 32)).is_err());
    }
}

#[cfg(test)]
#[path = "external_texture_tests.rs"]
mod gpu_tests;
