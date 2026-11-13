# Kairotensor

Kairotensor is a Rust framework for versioned evidence memory. Its first component is a local SQLite store that records which observations an agent used and checks whether a saved result is still eligible to read or resume after evidence changes.

## Run the withdrawal example

With stable Rust, Cargo, and a C compiler for bundled SQLite, run these commands from the repository root.

```sh
cargo run --example withdrawal
cargo test --all-targets
```

The example counts 8 items on one shelf and 4 on another. It starts a second count, then withdraws the second shelf report. The previous total of 12 becomes unavailable for both current reads and resume. The late count still costs two additions. A fresh attempt over the same candidate list sees only the first shelf and publishes 8. The ledger records five additions across all three attempts.

## Implemented contracts

| Operation | Behavior |
| --- | --- |
| `observe`, `set_admitted` | Record a distinct observation occurrence and change visibility for one actor and scope. Repeating an admission leaves the view epoch unchanged. |
| `begin_from_candidates` | Select admitted inputs from an ordered list and freeze their bytes, the view epoch and the expected branch head in one transaction. This also covers a selection that returns zero inputs. |
| `begin_attempt` | Require every explicit input to be admitted. Record the ordered IDs and start fresh, with no inherited output. |
| `read_current`, `begin_resume` | Check the accepted head against the current view before returning bytes. A resume records which accepted output it inherited. |
| `settle` | Validate the captured branch and view, record the delivery and usage, and advance the head only on acceptance. Identical retries return the first disposition. Conflicting retries fail. |

The caller owns actor authentication, output validation and usage measurement. Settlement currently distinguishes acceptance, a stale view and a stale branch. Each branch belongs to one actor and scope. An effective admission or revocation in that scope conservatively invalidates the previous head. A fresh computation can reuse raw evidence that is still admitted. Withdrawal governs future reads from this store. Previously delivered bytes may still exist in a caller.

## Reference and next milestone

[The v0 design](docs/memory-v0.md) describes the larger research program. The Python reference under `reference/` covers additional proposed behavior, including alternative derivations and exact categorical belief revision. Its original fixtures run with the Python standard library.

```sh
python3 -m unittest discover -s reference -p test_memory.py
python3 reference/belief_oracle.py
```

The next milestone is selective revision. Given a withdrawn source, identify its consumers, preserve results with independently validated support, and check the revised deterministic output against recomputation from the remaining evidence. Model adapters, forecasting, and tensor representations follow that baseline.
