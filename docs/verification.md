# Guard initial slice verification

Verified on 19 October 2026 with Rust/Cargo 1.92.0 on macOS.

- `cargo fmt --check`: passed.
- `cargo clippy --offline --locked --all-targets -- -D warnings`: passed.
- `cargo test --offline`: 11 library tests and 12 executable integration tests passed.
  The HTTP tests required loopback socket access outside the restricted sandbox.
- `cargo build --release --offline`: passed; executable is `target/release/guard`.
- Release offline audit: 6 submissions, 4 findings, 3 SMS bodies audited, 4 estimated
  segments, and 2 unavailable bodies. `audit.json` was generated from the fixture.
- Release offline explanation: the emoji example has 86 UTF-16 units and 2 segments;
  the original wording has 83 GSM-7 septets and 1 segment.

The existing sibling Java application's compiled classes were launched with Java
25.0.1 on a temporary loopback port and a separate in-memory H2 database. No Java
source files or existing persistent databases were changed.

1. The read-only `scripts/verify_live.py` check matched all 61 initial Java
   submission IDs, receipt timestamps, outcomes, and resolved channels; estimated
   18 SMS segments; and verified unchanged Java statistics.
2. Test setup submitted three synthetic SMS messages to that isolated Java process:
   the plain fixture wording, the same wording plus emoji, and a body containing
   `{{firstName}}` and `${order.id}`.
3. The release executable audited all 64 retained submissions. The new plain and
   emoji messages estimated 1 and 2 segments respectively. The template finding
   reported both placeholders. Totals were 3 findings across 2 submissions and
   22 estimated segments across 21 SMS bodies.
4. The read-only verification script again matched every ID and metadata field,
   exercised `explain`, and verified unchanged Java statistics during auditing.

Java was stopped after verification. The repeatable offline fixture remains in
this checkout. IntelliJ run-configuration XML is supplied, but the IDE run/debug
interaction was not exercised. Insights import/chart changes, a Pulse SMS campaign,
content-length policies, and demo secret patterns are outside this initial slice.
