# Safety and Recovery

## High-risk operations

- Any write operation to clocks, voltage/frequency points, power limits, fan settings.
- Bulk scan loops without thermal/power observation.

## Recovery controls

- TDR registry strategy (Windows) for driver timeout behavior.
- `auto-optimizer/systemd/` units and scripts for controlled startup/recovery on Linux.
- Reset command path (`reset`) as first-line rollback.

## Do-not-OC cases

- Unknown cooling or unstable PSU.
- Production workload without maintenance window.
- Unsupported/untested GPU + backend combination.

---

*Maintained from: `auto-optimizer/README.md`, `auto-optimizer/systemd/`, platform recovery docs/scripts.*
