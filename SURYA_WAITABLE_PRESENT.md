# Waitable presentation experiment

`SURYA_PRESENT_WAITABLE=1` creates a waitable DXGI swap chain. Unset keeps
the existing creation flags and frame callback behavior. This head also
contains the device queue-limit control, with a separate opt-in flag.
Both flags are off by default; neither enables the other.

The existing `VSyncProvider` already calls `DwmFlush` and invalidates each
window. This experiment retains that provider and `Present(0, 0)`, then
checks swap-chain readiness before asking GPUI to build the next frame.
It does not add another `DwmFlush` call to the application thread.

The gate waits for the DXGI handle or pending messages, with a bounded
timeout. Pending input returns control to the message loop. A deferred
paint validates its current region so it cannot spin on `WM_PAINT`; the
existing provider requests another paint on the next display beat. A
deferred forced render stays pending, including after device recovery.

An acquired permit lasts until a successful `Present` call. This matters
because GPUI can visit an unchanged view without presenting anything. The
first actual frame also obtains a permit. The wait handle closes before
the swap chain is dropped; device recovery creates a new gate. Both swap
chain creation paths and `ResizeBuffers` use the same immutable flag.

Waitable swap chains have their own queue latency. The startup line reports
DXGI's readback; this patch does not change it with the device-level setter.
Consequently its comparison must stand alone, with the device-latency,
threaded CEF, zero-copy, clock, and frame-rate switches controlled explicitly.

Logs count readiness waits, acquired permits, message interruptions,
timeouts, failed waits, successful `Present` calls, and wait durations.
These are submission and scheduling counters; they do not measure physical
display scans. Browser paint-to-surface timing excludes this earlier wait,
so report the gate's wait totals separately when comparing application work.

Validation: the new module compiled in the Windows API probe using this
fork's `windows` 0.61.3 dependency (`WINPROBE_EXIT=0`). The independent
present integration at app revision
`b0c43c0cd368039694a51d316fa6bdbbce3129f2` passed a Windows release build
with `--locked` (`BUILD_EXIT=0`). The combined head's application build,
first-frame/idle/resize/recovery checks, and matched dtry numbers remain
pending. No performance improvement has been established.

API contracts:

- [DXGI waitable swap chains](https://learn.microsoft.com/en-us/windows/uwp/gaming/reduce-latency-with-dxgi-1-3-swap-chains)
- [Wait-handle ownership](https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_3/nf-dxgi1_3-idxgiswapchain2-getframelatencywaitableobject)
- [Message-aware waiting](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-msgwaitformultipleobjectsex)
