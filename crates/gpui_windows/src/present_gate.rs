use std::{sync::OnceLock, time::Instant};

use anyhow::{Context, Result, bail};
use windows::{
    Win32::{
        Foundation::{CloseHandle, GetLastError, HANDLE, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT},
        Graphics::Dxgi::{
            DXGI_SWAP_CHAIN_FLAG, DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT,
            IDXGISwapChain1, IDXGISwapChain2,
        },
        UI::WindowsAndMessaging::{
            MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx, QS_ALLINPUT, QS_PAINT,
        },
    },
    core::Interface,
};

pub(crate) fn flags() -> DXGI_SWAP_CHAIN_FLAG {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    if *ENABLED
        .get_or_init(|| std::env::var("SURYA_PRESENT_WAITABLE").is_ok_and(|value| value == "1"))
    {
        DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT
    } else {
        DXGI_SWAP_CHAIN_FLAG(0)
    }
}

pub(crate) struct PresentGate {
    handle: HANDLE,
    permitted: bool,
    asked: u64,
    ready: u64,
    input: u64,
    timeout: u64,
    failed: u64,
    presents: u64,
    total_us: u64,
    max_us: u64,
}

impl PresentGate {
    pub(crate) fn new(swap_chain: &IDXGISwapChain1) -> Result<Option<Self>> {
        if flags().0 == 0 {
            return Ok(None);
        }
        let chain: IDXGISwapChain2 = swap_chain.cast().context("Waitable swap chain interface")?;
        let latency = unsafe { chain.GetMaximumFrameLatency() }
            .context("Reading waitable swap chain latency")?;
        let handle = unsafe { chain.GetFrameLatencyWaitableObject() };
        if handle.is_invalid() {
            bail!("Waitable swap chain returned an invalid handle");
        }
        log::info!("present_waitable=on queue_latency={latency}");
        Ok(Some(Self {
            handle,
            permitted: false,
            asked: 0,
            ready: 0,
            input: 0,
            timeout: 0,
            failed: 0,
            presents: 0,
            total_us: 0,
            max_us: 0,
        }))
    }

    pub(crate) fn before_frame(&mut self) -> bool {
        // WM_PAINT also visits unchanged views. Keep a consumed signal until
        // an actual Present, otherwise an idle window waits for work it never queued.
        if self.permitted {
            return true;
        }
        self.asked += 1;
        let started = Instant::now();
        // Let the message loop process pending input instead of blocking it
        // behind the GPU. The existing VSyncProvider requests another paint.
        // Exclude this WM_PAINT's own still-invalid region from the wake mask.
        let result = unsafe {
            MsgWaitForMultipleObjectsEx(
                Some(&[self.handle]),
                16,
                QS_ALLINPUT & !QS_PAINT,
                MWMO_INPUTAVAILABLE,
            )
        };
        let failure = (result == WAIT_FAILED).then(|| unsafe { GetLastError() });
        let elapsed = started.elapsed().as_micros() as u64;
        self.total_us += elapsed;
        self.max_us = self.max_us.max(elapsed);
        match result {
            WAIT_OBJECT_0 => {
                self.ready += 1;
                self.permitted = true;
            }
            WAIT_TIMEOUT => self.timeout += 1,
            WAIT_FAILED => {
                self.failed += 1;
                log::error!("present_waitable wait failed: {failure:?}");
            }
            value if value.0 == WAIT_OBJECT_0.0 + 1 => self.input += 1,
            value => {
                self.failed += 1;
                log::error!("present_waitable unexpected wait result: {value:?}");
            }
        }
        self.permitted
    }

    pub(crate) fn presented(&mut self) {
        self.permitted = false;
        self.presents += 1;
        if self.presents % 240 == 0 {
            self.report();
        }
    }

    fn report(&self) {
        log::info!(
            "present_waitable asked={} ready={} input={} timeout={} failed={} presents={} wait_total_us={} wait_max_us={}",
            self.asked,
            self.ready,
            self.input,
            self.timeout,
            self.failed,
            self.presents,
            self.total_us,
            self.max_us,
        );
    }
}

impl Drop for PresentGate {
    fn drop(&mut self) {
        self.report();
        if let Err(error) = unsafe { CloseHandle(self.handle) } {
            log::error!("Closing presentation wait handle: {error}");
        }
    }
}
