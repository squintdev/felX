# Gain

The simplest effect: multiplies every output channel by a scalar. Useful for sanity-checking the pipeline or as the starting point for any custom effect.

| Parameter | Type | Default | Range | Notes |
|---|---|---|---|---|
| `gain` | float | 1.0 | 0.0…4.0 | Multiplied per channel; alpha untouched. |

**Pass type:** GPU. Hot-reloadable.
**Working space:** linear (gamma-aware multiplication is gain on linear values).
