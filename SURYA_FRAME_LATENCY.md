# DXGI queue-limit experiment

`SURYA_FRAME_LATENCY=1` requests a maximum device queue latency of one.
Unset does not call the DXGI setter. Values outside 1 through 16 are rejected.
The startup log includes the requested value and the value read back from DXGI.
A failed setter/readback is logged and does not prevent the application opening.

This ports haktui's `gpui-zedmain-f42c6e87-frame-latency.patch` to the Surya
fork. The difference is deliberate: haktui defaulted to one; this experiment
keeps the previous policy unless explicitly enabled, pending measurements on
the owner's Windows machine. It runs on device construction and recovery.

The [Microsoft API contract](https://learn.microsoft.com/en-us/windows/win32/api/dxgi/nf-dxgi-idxgidevice1-setmaximumframelatency)
describes a queue limit, not a guarantee of a higher frame rate. The app must
measure fresh browser surface submissions and paint-to-draw latency against
its unchanged control. `Present` timing and the device queue limit are separate
levers, controlled independently even though this head contains both.

Validation so far: the module compiled for `x86_64-pc-windows-msvc` in the
Astra API probe (`windows` 0.61.3, matching this fork's 0.61 dependency,
`WINPROBE_EXIT=0`). The independent latency integration at app revision
`22caa8129706500380995c453eddc0d81a7ebeb3` passed a Windows release build
with `--locked` (`BUILD_EXIT=0`). The combined head's application build,
Windows runtime readback, and matched dtry measurements are pending.
These build checks establish compilation, not a performance benefit.
