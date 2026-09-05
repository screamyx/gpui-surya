use std::{cell::Cell, rc::Rc, sync::OnceLock, time::Instant};

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

use crate::present_gate_state::{PermitState, enabled};

pub(crate) fn flags() -> DXGI_SWAP_CHAIN_FLAG {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    if *ENABLED.get_or_init(|| enabled(std::env::var("SURYA_PRESENT_WAITABLE").ok().as_deref())) {
        DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT
    } else {
        DXGI_SWAP_CHAIN_FLAG(0)
    }
}

struct WaitHandle(HANDLE);

impl Drop for WaitHandle {
    fn drop(&mut self) {
        if let Err(error) = unsafe { CloseHandle(self.0) } {
            log::error!("Closing presentation wait handle: {error}");
        }
    }
}

#[derive(Clone, Copy, Default)]
struct Counters {
    asked: u64,
    ready: u64,
    input: u64,
    timeout: u64,
    stale: u64,
    failed: u64,
    presents: u64,
    unused: u64,
    modal: u64,
    total_us: u64,
    max_us: u64,
}

pub(crate) struct PresentGate {
    // A re-entrant device recovery can drop the renderer's resources while
    // waiting. This Rc owns both objects; close the handle before the chain.
    handle: WaitHandle,
    _chain: IDXGISwapChain2,
    state: Cell<PermitState>,
    generation: Cell<u64>,
    counters: Cell<Counters>,
}

pub(crate) struct FramePermit(Rc<PresentGate>);

impl Drop for FramePermit {
    fn drop(&mut self) {
        let mut state = self.0.state.get();
        if state.finish() {
            let mut counters = self.0.counters.get();
            counters.unused += 1;
            self.0.counters.set(counters);
        }
        self.0.state.set(state);
    }
}

impl PresentGate {
    pub(crate) fn new(swap_chain: &IDXGISwapChain1) -> Result<Option<Rc<Self>>> {
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
        log::info!(
            "present_waitable=on effective_scope=swap_chain effective_queue_latency={latency}"
        );
        Ok(Some(Rc::new(Self {
            handle: WaitHandle(handle),
            _chain: chain,
            state: Cell::default(),
            generation: Cell::new(0),
            counters: Cell::default(),
        })))
    }

    pub(crate) fn begin_frame(
        self: &Rc<Self>,
        timeout_ms: u32,
        modal: bool,
    ) -> Option<FramePermit> {
        if !modal && self.state.get().needs_wait() {
            let generation = self.generation.get();
            let started = Instant::now();
            // No renderer/state borrow is held here. Sent window messages
            // may re-enter; posted input returns to the outer message loop.
            let result = unsafe {
                MsgWaitForMultipleObjectsEx(
                    Some(&[self.handle.0]),
                    timeout_ms,
                    QS_ALLINPUT & !QS_PAINT,
                    MWMO_INPUTAVAILABLE,
                )
            };
            let failure = (result == WAIT_FAILED).then(|| unsafe { GetLastError() });
            let elapsed = started.elapsed().as_micros() as u64;
            let mut counters = self.counters.get();
            counters.asked += 1;
            counters.total_us += elapsed;
            counters.max_us = counters.max_us.max(elapsed);
            let mut state = self.state.get();
            match result {
                WAIT_OBJECT_0 if generation == self.generation.get() => {
                    counters.ready += 1;
                    state.ready();
                }
                WAIT_OBJECT_0 => counters.stale += 1,
                WAIT_TIMEOUT => counters.timeout += 1,
                WAIT_FAILED => {
                    counters.failed += 1;
                    state.failed();
                    log::error!("present_waitable wait failed: {failure:?}");
                }
                value if value.0 == WAIT_OBJECT_0.0 + 1 => counters.input += 1,
                value => {
                    counters.failed += 1;
                    state.failed();
                    log::error!("present_waitable unexpected wait result: {value:?}");
                }
            }
            self.state.set(state);
            self.counters.set(counters);
        }
        let mut state = self.state.get();
        if !state.begin(modal) {
            return None;
        }
        self.state.set(state);
        if modal {
            let mut counters = self.counters.get();
            counters.modal += 1;
            self.counters.set(counters);
        }
        Some(FramePermit(self.clone()))
    }

    pub(crate) fn presented(&self) {
        let mut state = self.state.get();
        state.presented();
        self.state.set(state);
        let mut counters = self.counters.get();
        counters.presents += 1;
        self.counters.set(counters);
        if counters.presents % 240 == 0 {
            self.report();
        }
    }

    pub(crate) fn is_faulted(&self) -> bool {
        self.state.get().is_faulted()
    }

    pub(crate) fn failed(&self) {
        let mut state = self.state.get();
        state.failed();
        self.state.set(state);
        let mut counters = self.counters.get();
        counters.failed += 1;
        self.counters.set(counters);
    }

    pub(crate) fn resized(&self) {
        self.generation.set(self.generation.get().wrapping_add(1));
        let mut state = self.state.get();
        state.resized();
        self.state.set(state);
    }

    fn report(&self) {
        let c = self.counters.get();
        log::info!(
            "present_waitable asked={} ready={} input={} timeout={} stale={} failed={} presents={} unused_frames={} modal_frames={} wait_total_us={} wait_max_us={}",
            c.asked,
            c.ready,
            c.input,
            c.timeout,
            c.stale,
            c.failed,
            c.presents,
            c.unused,
            c.modal,
            c.total_us,
            c.max_us,
        );
    }
}

impl Drop for PresentGate {
    fn drop(&mut self) {
        self.report();
    }
}

#[cfg(test)]
mod tests {
    use super::WaitHandle;
    use windows::Win32::{Foundation::GetHandleInformation, System::Threading::CreateEventW};

    #[test]
    fn owned_wait_handle_is_closed_on_drop() -> windows::core::Result<()> {
        let handle = unsafe { CreateEventW(None, false, false, None) }?;
        let owned = WaitHandle(handle);
        let mut flags = 0;
        unsafe { GetHandleInformation(handle, &mut flags) }?;
        drop(owned);
        assert!(unsafe { GetHandleInformation(handle, &mut flags) }.is_err());
        Ok(())
    }
}
