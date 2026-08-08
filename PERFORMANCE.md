# Performance baseline and resource model

This document records the baseline observed before the source-resolution rendering and Alpha
readback work, plus the expected resource model after it. `N = width * height` pixels and `k` is
the window scale relative to the sender.

## Verification baseline

- `cargo test`: 18 tests passed.
- `cargo clippy --all-targets --all-features -- -D warnings`: passed.
- `cargo build --release`: passed.
- The only toolchain message was Cargo being unable to canonicalize the parent user-profile path;
  it was not a compiler, dependency, or Clippy warning.

## GPU memory

Before this change, the application-owned display and Alpha resources were approximately:

```text
2 BGRA swapchain buffers + 3 BGRA staging textures
= (2 * 4 + 3 * 4) * k^2 * N
= 20 * k^2 * N bytes
```

After this change, the swapchain remains at sender resolution and Alpha uses one R8 render target
plus two R8 staging textures:

```text
2 BGRA swapchain buffers + 1 R8 target + 2 R8 staging textures
= (2 * 4 + 1 + 2) * N
= 11 * N bytes while interaction is enabled
```

When interaction is disabled, the R8 resources are released, leaving approximately `8 * N`
bytes. These formulas exclude the sender-owned shared texture, driver alignment, and compositor
surfaces.

| Sender size | Old at 1x | New, interaction on | New, interaction off |
| --- | ---: | ---: | ---: |
| 1920x1080 | 39.55 MiB | 21.75 MiB | 15.82 MiB |
| 3840x2160 | 158.20 MiB | 87.01 MiB | 63.28 MiB |

At 2x display scale, the old 1080p resource model grew to about 158.20 MiB; the new model remains
21.75 MiB because only the DirectComposition transform and window size change.

Considering Alpha resources alone, storage drops from `12 * N` to `3 * N` bytes (75%):

| Sender size | Old Alpha resources | New Alpha resources |
| --- | ---: | ---: |
| 1920x1080 | 23.73 MiB | 5.93 MiB |
| 3840x2160 | 94.92 MiB | 23.73 MiB |

## Readback and rendering bandwidth

The CPU-visible Alpha transfer per sample drops from BGRA `4 * N` to R8 `N` bytes. At the current
10 Hz Alpha update interval, the nominal staging/readback traffic changes as follows:

| Sender size | Old | New |
| --- | ---: | ---: |
| 1920x1080 | 79.10 MiB/s | 19.78 MiB/s |
| 3840x2160 | 316.41 MiB/s | 79.10 MiB/s |

The R8 path adds a source-resolution pixel-shader pass, so total GPU traffic also includes reading
the source and writing the R8 target. The table intentionally reports only the staging/readback
portion with an unambiguous format-size reduction.

Application Draw work now stays at `N` pixels rather than `k^2 * N`; DirectComposition performs
the final display scaling. When the Spout frame-count semaphore is active, Draw/Present frequency
is bounded by new sender frames (plus explicit redraws). Senders that leave the count at zero keep
the previous receiver-paced behavior.
