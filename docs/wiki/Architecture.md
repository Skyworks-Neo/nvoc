# Architecture

```text
CLI / Frontends (GUI, TUI, SRV)
          ↕
       pynvoc
          ↕
      nvoc-core
      ↙      ↘
   NVAPI     NVML
```

## Path selection

- CLI path: direct `auto-optimizer` execution for scripting, autoscan, and low-level operations.
- Native frontend path: GUI/TUI/SRV orchestrate user workflows and invoke CLI/backend paths with validated config.

Use CLI for reproducible automation and troubleshooting; use frontends for operator UX and guardrails.

---

*Maintained from: `README.md`, `nvoc-core/src/lib.rs`, `gui/src/`, `tui/nvoc_tui/`, `srv/src/`.*
