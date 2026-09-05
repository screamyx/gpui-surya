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
levers; this patch changes only the latter.

Validation so far: the module compiled for `x86_64-pc-windows-msvc` in the
Astra API probe (`windows` 0.62.2, `WINPROBE_EXIT=0`). Full application build,
Windows runtime readback, and the paired cadence measurements are pending.
Do not treat the cross compile as RC proof.
