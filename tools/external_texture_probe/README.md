# External D3D11 texture probe

This standalone Windows program creates an opaque BGRA8 NT shared texture,
fills it with a GPU clear, observes GPU completion, and passes its owner to
`Window::paint_external_texture`. The green rectangle is the external texture.
The dark background is an ordinary GPUI quad. The producer device and its
texture wrappers are dropped before the window renders; the owned NT handle
keeps the allocation alive.

From an interactive Windows desktop:

```powershell
$env:GPUI_EXTERNAL_TEXTURE_STATS = '1'
cargo check --manifest-path tools/external_texture_probe/Cargo.toml
cargo build --manifest-path tools/external_texture_probe/Cargo.toml
cargo run --manifest-path tools/external_texture_probe/Cargo.toml
cargo test --manifest-path tools/external_texture_probe/Cargo.toml -p gpui_windows --lib external_texture -- --nocapture
```

An SSH session alone cannot create the GPUI window. On a remote test machine,
run the executable in its interactive session, for example through a scheduled
task. Set isolated Cargo home/target paths on a shared machine.

## Contract

`gpui::ExternalTexture::new(OwnedHandle, (LowPart, HighPart), (width, height), sync)`
is unsafe: the caller guarantees an opaque, single-mip/layer/sample BGRA8
texture and obeys the lifetime and synchronization documentation.

`ProducerComplete` means GPU writes have finished and content stays immutable.
For this mode create with `SHARED | SHARED_NTHANDLE`. A fresh CEF snapshot must
be copied and completed inside `OnAcceleratedPaint`; its pooled source cannot
be used after that callback. A duplicated handle does not freeze pooled pixels.

`KeyedMutex` means every access acquires key 0 and releases key 0. Create with
`SHARED_KEYEDMUTEX | SHARED_NTHANDLE`. The renderer tests acquisition without
blocking and skips the surface on any result other than exact `S_OK`, including
the positive HRESULT values `WAIT_TIMEOUT` and `WAIT_ABANDONED`.

The scene retains a clone of the handle owner. The renderer checks adapter LUID
and actual format/size, then copies the intersection of bounds and source pixels.
Opened textures are cached by weak Arc owner identity and device, never by
handle value. Retired owners are pruned even on frames without surfaces.
This first entry is opaque and rectangular with device pixels copied 1:1.
DPI rounding or resize differences are cropped without rejecting the surface. It
does not implement scaling, corner radii, global opacity, or edge fades. No
CPU pixels, sprite-atlas uploads, GPU-facing struct changes, or shader changes.

Reference-count uniqueness is not a GPU completion fence. Do not recycle a
`ProducerComplete` resource after its CPU scene clone drops: submitted GPU work
may still be reading it. A future reusable pool needs explicit synchronization.

## Measured on dtry, 2026-09-05

- Windows Rust/Cargo 1.97.1: check and build exited 0.
- GUI renderer: `asked=59 frames=59 clipped=0 dropped=0`.
- PrintWindow: `asked=1 done=1`, screenshot inspected; green rectangle visible.
- Tests: `5 passed; 0 failed`, including `asked=32 frames=32 dropped=0` and
  `pixels_asked=256 pixels_matched=256` in the GPU clipping/ownership test.
- The mutex test verifies a key-0 timeout cannot count as an acquired lock.
- The readback test also covers bounds larger than the available texture and
  verifies cache reuse plus retirement after the final owner drops.

These counters prove the synthetic texture entry. CEF page rendering, callback
cost, resizing under load, and default CPU fallback are separate application
integration proofs. Submission timing is not end-to-end GPU completion time.
