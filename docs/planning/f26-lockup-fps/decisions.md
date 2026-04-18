# Decisions - F26 Post-Boot Lockup + Bounce FPS Regression

## 2026-04-18 - Initial Scope

**Choice:** Treat Phase 1 as measurement-only and avoid code changes until the lockup class is known.
**Alternatives considered:** Remove the bounce sleep immediately because it is a likely FPS cause.
**Evidence:** The lockup may be independent of FPS, and removing the sleep first could mask or worsen the original failure mode.
