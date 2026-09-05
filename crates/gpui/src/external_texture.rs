//! Owned Windows textures whose pixels never pass through the sprite atlas.

use std::{
    os::windows::io::{AsRawHandle, OwnedHandle, RawHandle},
    sync::{Arc, Weak},
};

/// Synchronization required before the renderer reads a shared texture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExternalTextureSync {
    /// The producer observed GPU completion and will never modify this texture.
    ProducerComplete,
    /// Both devices acquire key 0 before access and release key 0 afterwards.
    KeyedMutex,
}

/// A client-owned NT shared D3D11 texture, retained by every scene that uses it.
///
/// This owns the NT handle, not a borrowed CEF pool handle. CEF producers must
/// open and GPU-copy their pool texture during OnAcceleratedPaint, then observe
/// copy completion before returning. Duplicating a CEF handle keeps the handle
/// alive but does not prevent pool reuse from changing its contents.
///
/// Clones share ownership; the final drop closes the NT handle. Resizing or
/// closing a browser can drop its owner immediately: retained scenes keep their
/// own clone. No frame-count retirement or cache keyed by handle value is safe.
#[derive(Clone, Debug)]
pub struct ExternalTexture {
    handle: Arc<OwnedHandle>,
    adapter_luid: (u32, i32),
    size: (u32, u32),
    synchronization: ExternalTextureSync,
}

impl ExternalTexture {
    /// Adopt an application-owned NT shared texture handle.
    ///
    /// `adapter_luid` is `(LowPart, HighPart)` and `size` is in device pixels.
    /// The renderer validates both against its device and the opened descriptor.
    ///
    /// # Safety
    ///
    /// The handle must refer to a single-mip, single-sample, single-layer,
    /// opaque BGRA8 UNORM D3D11 texture on the specified adapter. The producer
    /// must obey `synchronization` for the entire lifetime of every clone.
    /// ProducerComplete requires completed GPU writes and immutable content.
    /// KeyedMutex requires all reads and writes to acquire key 0 and release
    /// key 0, with the resource dimensions and format remaining fixed.
    pub unsafe fn new(
        handle: OwnedHandle,
        adapter_luid: (u32, i32),
        size: (u32, u32),
        synchronization: ExternalTextureSync,
    ) -> Self {
        Self {
            handle: Arc::new(handle),
            adapter_luid,
            size,
            synchronization,
        }
    }

    /// Borrow the NT handle. It remains valid while this owner is alive.
    pub fn as_raw_handle(&self) -> RawHandle {
        self.handle.as_raw_handle()
    }

    /// A weak owner for backend caches, which must not extend scene lifetime.
    #[doc(hidden)]
    pub fn downgrade_handle(&self) -> Weak<OwnedHandle> {
        Arc::downgrade(&self.handle)
    }

    /// The originating adapter's `(LowPart, HighPart)` LUID.
    pub fn adapter_luid(&self) -> (u32, i32) {
        self.adapter_luid
    }

    /// Width and height in device pixels.
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// The producer's synchronization contract.
    pub fn synchronization(&self) -> ExternalTextureSync {
        self.synchronization
    }
}
