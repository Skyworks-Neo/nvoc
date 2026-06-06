# Contributing

1. Keep changes scoped by component (`auto-optimizer`, `core`, `gui`, `tui`, `srv`, stressors).
2. Run local command set aligned with CI for touched paths.
3. For GPU-affecting changes, document hardware assumptions and safety behavior.
4. Use path filters to avoid unnecessary GPU CI, and trigger GPU CI only when relevant paths or labels require it.

## PR checklist

- Component + behavior summary
- Linked issue(s)
- Tests/lints/build commands run
- GPU availability note and any skipped GPU-only checks

---

*Maintained from: `CONTRIBUTING.md`, CI workflow path filters, component README test sections.*
