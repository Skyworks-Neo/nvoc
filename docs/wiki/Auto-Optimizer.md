# Auto Optimizer

## Key workflows

- `autoscan`: automatic stability scan loop over core/memory deltas.
- `vfp`: V-F curve read/export/import operations.
- `reset`: reset clocks/power/fan-related state to safe baseline.
- `fix_result`: normalize and patch scan outputs for downstream use.

## Operational notes

- Prefer read-only checks before mutating GPU state.
- Pair autoscan with an appropriate stressor module.
- Keep recovery scripts and reboot strategy prepared for failed OCs.

---

*Maintained from: `auto-optimizer/README.md`, `auto-optimizer/README-en.md`.*
