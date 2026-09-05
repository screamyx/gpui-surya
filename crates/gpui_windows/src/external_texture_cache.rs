use std::{collections::HashMap, os::windows::io::OwnedHandle, sync::Weak};

use anyhow::{Context, Result, ensure};
use gpui::ExternalTexture;
use windows::{
    Win32::{Foundation::HANDLE, Graphics::Direct3D11::*},
    core::Interface,
};

use crate::directx_renderer::DirectXRendererDevices;

pub(crate) struct OpenedTexture {
    owner: Weak<OwnedHandle>,
    pub texture: ID3D11Texture2D,
    pub descriptor: D3D11_TEXTURE2D_DESC,
}

#[derive(Default)]
pub(crate) struct ExternalTextureCache {
    device: Option<ID3D11Device>,
    opener: Option<ID3D11Device1>,
    adapter_luid: Option<(u32, i32)>,
    entries: HashMap<usize, OpenedTexture>,
}

impl ExternalTextureCache {
    pub(crate) fn prune(&mut self) {
        self.entries
            .retain(|_, entry| entry.owner.strong_count() > 0);
        if self.entries.is_empty() {
            self.device = None;
            self.opener = None;
            self.adapter_luid = None;
        }
    }

    pub(crate) fn get(
        &mut self,
        devices: &DirectXRendererDevices,
        owner: &ExternalTexture,
    ) -> Result<&OpenedTexture> {
        if self.device.as_ref() != Some(&devices.device) {
            self.entries.clear();
            self.device = None;
            self.opener = None;
            self.adapter_luid = None;
            let adapter = unsafe { devices.adapter.GetDesc1() }?;
            self.opener = Some(devices.device.cast()?);
            self.adapter_luid = Some((adapter.AdapterLuid.LowPart, adapter.AdapterLuid.HighPart));
            self.device = Some(devices.device.clone());
        }
        ensure!(
            Some(owner.adapter_luid()) == self.adapter_luid,
            "external texture adapter LUID differs from renderer"
        );
        let weak = owner.downgrade_handle();
        // The Weak keeps this Arc allocation's identity reserved, even after
        // its handle closes. Kernel HANDLE values can be recycled immediately.
        let key = weak.as_ptr() as usize;
        if let std::collections::hash_map::Entry::Vacant(entry) = self.entries.entry(key) {
            let opener = self
                .opener
                .as_ref()
                .context("external texture device missing")?;
            let texture = unsafe {
                opener.OpenSharedResource1::<ID3D11Texture2D>(HANDLE(owner.as_raw_handle()))
            }
            .context("open owned NT texture")?;
            let mut descriptor = D3D11_TEXTURE2D_DESC::default();
            unsafe {
                texture.GetDesc(&mut descriptor);
            }
            entry.insert(OpenedTexture {
                owner: weak,
                texture,
                descriptor,
            });
        }
        self.entries
            .get(&key)
            .context("external texture cache entry missing")
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}
