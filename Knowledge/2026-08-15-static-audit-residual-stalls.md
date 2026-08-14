# Static Audit — Residual Render and Tray Stalls

- Repository: `StarrySky7D4/spout-transparent`
- Source: `AutoSave`
- Target: `main`
- Reviewed base: `1efa781a0f509a4558da3bda4cefaf51db69a99e`
- Reviewed code head: `e1041b6c8399e18da966eecfd11346a331fcbc5d`
- Review date: 2026-08-15
- Review mode: static source/diff review only; GitHub Actions were not invoked or used as a merge condition.

## Scope

The change removes blocking Spout discovery and metadata reads from the render loop, queues passthrough mouse messages instead of synchronously calling another window procedure, stabilizes tray source selection, constrains render scaling, and resets interaction state when visibility or source resources change.

## Findings

1. **Discovery no longer blocks the render loop.** Sender enumeration runs on a dedicated worker. The UI consumes a single latest snapshot under a short mutex hold.
2. **Realtime metadata polling is non-blocking.** The render path uses a zero-timeout metadata mutex and treats `WouldBlock` as a transient condition while retaining the current sender state.
3. **Worker shutdown is bounded by the active discovery operation.** `Drop` signals the worker and joins it; the only intentional blocking metadata read is limited to the discovery worker's 67 ms timeout.
4. **Tray selection avoids index drift.** The visible menu snapshot resolves a command to an owned `SenderName` before publishing the action, so a later discovery refresh cannot redirect the command to another sender.
5. **Mouse passthrough removes synchronous foreign-window entry.** The forwarded mouse messages contain value-only parameters, making queued `PostMessageW` use safe from borrowed-pointer lifetime issues.
6. **Interaction state is reset at invalidation boundaries.** Hiding the window, disabling interaction, and replacing the sender cancel dragging; source replacement also invalidates the alpha mask.
7. **Scale limiting preserves the render-dimension cap.** The effective scale is clamped before swapchain/window resizing, and source aspect ratio remains unchanged.

## Residual risks and follow-up

- `PostMessageW` changes delivery from synchronous to queued. A saturated or terminating target queue may reject a message; the current code intentionally ignores that result to keep rendering responsive. Rate-limited diagnostics may be useful if field reports show lost clicks.
- The discovery worker and Win32 message behavior were reviewed statically but were not exercised on a Windows desktop in this review.
- The source includes focused unit tests for scale constraints, drag cancellation, and menu snapshot resolution; their results were not independently reproduced here.

## Decision

**Approved for squash merge into `main`.** The reviewed changes directly address the reported stall paths, preserve ownership across threads, and introduce no static correctness blocker.
