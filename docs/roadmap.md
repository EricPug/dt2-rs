# Development roadmap

These are proposed tasks based on current gaps in the repository. Effort values
are planning estimates, not completed work, guarantees, or commitments. Each
task depends on its predecessor only where stated; the hardware-evidence work
can proceed independently of core instruction coverage.

| Task | Work and completion evidence | Depends on | Estimate |
|---|---|---|---:|
| Expand ColdFire coverage | Implement and test priority instruction families using CPU unit tests and differential state checks. | — | 12 h |
| Improve peripheral models | Define model invariants and expand timer, interrupt-controller, eSDHC, and DMA regression tests against captured emulator behavior. | — | 14 h |
| Establish independent hardware evidence | Design repeatable observations on hardware owned by the researcher; record setup, raw measurements, and which emulator claims they validate. | Access to hardware and measurement setup | 12 h |
| Make diagnostic execution reproducible | Standardize inputs, output directories, run metadata, and trace summaries for documented diagnostic scenarios. | — | 8 h |
| Expand sample/UI regression coverage | Add deterministic tests for browser navigation, sample loading, input sequences, and modeled-DSP boundaries using synthetic fixtures. | Diagnostic execution reproducibility | 10 h |
| Document additional firmware paths | Trace another user-facing path from explicit inputs through modeled peripherals; document evidence, assumptions, and an executable reproduction. | Improved peripheral models | 10 h |
| Improve trace analysis | Add compact comparison summaries and reproducibility checks for interrupt, memory-mapped I/O, display, and sample-loading traces. | Diagnostic execution reproducibility | 8 h |

Estimated total: **74 hours**. Estimates may change after the prerequisites
and test fixtures are evaluated.
