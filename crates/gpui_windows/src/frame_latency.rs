//! Opt-in DXGI queue limit for the browser cadence experiment.
//!
//! Unset leaves the device's existing policy untouched. Keep this separate
//! from a present/vsync change so both levers can be measured independently.

use windows::Win32::Graphics::{Direct3D11::ID3D11Device, Dxgi::IDXGIDevice1};
use windows::core::Interface;

pub(crate) fn apply(device: &ID3D11Device) {
    let Ok(value) = std::env::var("SURYA_FRAME_LATENCY") else {
        return;
    };
    let Some(requested) = parse(&value) else {
        log::warn!("dxgi: SURYA_FRAME_LATENCY must be an integer from 1 through 16; unchanged");
        return;
    };
    let result = (|| -> windows::core::Result<u32> {
        let device: IDXGIDevice1 = device.cast()?;
        unsafe {
            device.SetMaximumFrameLatency(requested)?;
            device.GetMaximumFrameLatency()
        }
    })();
    match result {
        Ok(actual) => {
            if crate::present_gate::flags().0 != 0 {
                log::info!(
                    "dxgi: frame_latency asked={requested} device_readback={actual} effective_scope=swap_chain device_limit_effective=0; chain readback follows"
                );
            } else {
                log::info!(
                    "dxgi: frame_latency asked={requested} device_readback={actual} effective_scope=device effective_queue_latency={actual} applied={}",
                    u8::from(actual == requested)
                );
            }
        }
        Err(error) => log::warn!("dxgi: frame_latency asked={requested} unverified: {error}"),
    }
}

fn parse(value: &str) -> Option<u32> {
    value.trim().parse().ok().filter(|n| (1..=16).contains(n))
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn queue_limits_reject_invalid_values_instead_of_silently_changing_them() {
        for value in ["", "0", "17", "-1", "fast"] {
            assert_eq!(parse(value), None);
        }
        for value in 1..=16 {
            assert_eq!(parse(&format!(" {value} ")), Some(value));
        }
    }
}
