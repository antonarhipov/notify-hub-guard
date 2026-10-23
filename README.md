# NotifyHub Guard

A read-only Rust content auditor for the NotifyHub demo. **Go generates traffic, Java
processes it, Rust inspects the content, and Python reveals the patterns.**

```mermaid
flowchart LR
    G["Pulse · Go<br/>Generate traffic"] -->|"POST /api/notify"| J["NotifyHub · Java<br/>Process and retain notifications"]
    J -->|"History API"| R["Guard · Rust<br/>Audit notification content"]
    J -->|"History API"| P["Insights · Python<br/>Explore traffic patterns"]
    R -->|"Audit JSON contract"| P
```

This first version implements template checks and SMS segment estimation. It reads
Java's public history API using GET requests and produces a local report. Findings
describe retained content, including failed/rejected submissions when content is
available; they do not establish delivery, provider charges, or a security incident.

## Run

Requires Rust 1.88+ with Cargo. The lockfile pins the demo's dependencies.

```sh
cargo build --release --locked
./target/release/guard audit --url http://localhost:8080 --output audit.json
./target/release/guard explain --submission <submission-uuid>
```

Start the Java application in `../notify-hub` separately. Its base URL defaults to
`http://localhost:8080`; both commands accept `--url`, including a context path.
An audit without `--output` writes JSON to stdout; status and errors use stderr.
`--output` atomically replaces the report after the complete audit succeeds.
An error leaves an existing report untouched. Exit codes: **0** completed audit
(findings included), **1** runtime/input error, **2** invalid CLI arguments.

For an offline demonstration with no running services:

```sh
./target/release/guard audit --input fixtures/demo-history.json --output audit.json
./target/release/guard explain --input fixtures/demo-history.json \
  --submission 10000000-0000-0000-0000-000000000002
```

The six synthetic details contain a plain SMS, the same wording with emoji,
unresolved body/subject variables, and unavailable content. The first SMS needs
**one segment**; adding `😀` switches the whole body to UTF-16 and needs **two**.
The report contains **4 findings**, **3 SMS estimates**, and **4 total estimated
SMS segments**. Offline input is an array of Java detail objects, never a fallback
when Java is unavailable. `--input` and `--url` are mutually exclusive.

## Rules

| Rule | Severity | Behavior |
| --- | --- | --- |
| `unresolved_template` | warning | Detect `{{firstName}}` and `${order.id}` in captured subject and body; one finding per affected field, including occurrence count |
| `sms_unicode` | info | Explain why a recorded SMS body requires UTF-16 instead of GSM-7 |
| `sms_multipart` | warning | Explain an SMS estimate above one segment and the one-segment limit |

Template names start with an ASCII letter or underscore; remaining characters may
be ASCII letters, digits, underscores, dots, or hyphens. Whitespace around the name
is allowed. Repeated occurrences count separately. Incomplete delimiters, expressions,
and ordinary single braces are ignored. There is no template engine or escape syntax;
intentionally literal examples of valid placeholders are also flagged.

GSM-7 basic characters cost one septet. Extension characters (`^`, `{`, `}`, `\`,
`[`, `]`, `~`, `|`, `€`, and form feed) cost two. One non-GSM character switches the
entire message to UTF-16; supplementary characters such as `😀` cost two code units.
Combining marks and emoji sequences are measured as stored, without normalization
or transliteration. Limits are **160/153 septets** for single/multipart GSM-7 and
**70/67 UTF-16 units** for Unicode. Segments never split an escape or surrogate pair;
an empty body estimates zero segments. Provider-specific encoding, alternate headers,
national-language tables, and grapheme-cluster policies can change actual results.

Only `resolvedChannel == "sms"` enables SMS checks. A requested SMS with no resolved
channel remains unattributed. Subject text is checked for templates on every channel;
SMS estimation uses only the body. Null, missing, and non-string text is skipped;
`body_available: false` and summary coverage counts make missing body evidence explicit.

## Java history boundary

Guard follows `/api/submissions?limit=200` and the opaque, URL-encoded `nextCursor`,
then reads `/api/submissions/{submissionId}` for each detail's `payload.subject` and
`payload.body`. Each submission is counted once; attempts add no counts. Source
outcomes, receipt timestamps, and resolved channels are preserved.

Like Insights, the default limit is **5,000 submissions**. `--max-records N` changes
it; exceeding the limit fails instead of silently truncating. HTTP requests have a
3-second connection timeout, a 15-second total timeout, and a 2 MiB response limit.
There are no retries or redirects. Invalid pages, repeated cursors/IDs, HTTP errors,
and mismatched detail IDs fail the audit. Details are loaded sequentially to keep
this demo small; large histories take longer.

Separate HTTP reads do not form a transactional snapshot. Use an idle Java instance
for a reproducible presentation. Guard does not submit, resend, edit content, or
open Java's database. Java may reconcile abandoned processing rows while serving
its own history endpoints. Reports omit recipients, subject/body content, and matched
variable names; findings retain only the field, rule, severity, and explanation.

## JSON handoff to Insights

Reports use **`schema_version: 1`** and snake_case keys. Every inspected submission
appears in `submissions`, even when it has no findings:

```json
{
  "submission_id": "10000000-0000-0000-0000-000000000002",
  "submitted_at": "2026-10-19T11:00:00Z",
  "outcome": "ACCEPTED",
  "resolved_channel": "sms",
  "body_available": true,
  "sms": {
    "encoding": "utf16",
    "units": 86,
    "segments": 2,
    "single_segment_limit": 70,
    "multipart_segment_limit": 67
  },
  "findings": []
}
```

This abbreviated example omits the two findings for readability. Each finding has
`submission_id`, `rule`, `severity`, `field`, and `explanation`. Encoding values are
`gsm7` or `utf16`; `sms: null` means no estimate was possible/applicable.

The root also contains `source` (the supplied Java URL or `file:...`) and `summary`:
`submissions`, `submissions_with_findings`, `bodies_unavailable`, `findings`,
`sms_submissions` (all resolved SMS), `sms_bodies_audited` (available SMS bodies),
and `estimated_sms_segments` (sum of available estimates across all outcomes).
These are content estimates, not billed/sent segment totals.

Insights can join `submissions[].submission_id` to Java's `submissionId`, then group
`sms.segments` by receipt hour or its own campaign classification. The Java API has
no campaign ID. Python import and chart enrichment are a follow-up in the Insights
project; this checkout provides the report contract. Pulse's current campaign is
mostly email and does not plant emoji; the offline fixture demonstrates the SMS
change until an SMS scenario is added there.

## IntelliJ demonstration

Open this project with Rust support and a Rust toolchain configured. Shared Cargo
run configurations are in `.run/`: **Guard · Offline audit**, **Guard · Explain
emoji**, and **Guard · Java history audit**.

1. Run the offline audit and open `audit.json`. Compare the first two `sms` objects.
2. Run **Explain emoji** to see the encoding and multipart explanations.
3. Put a breakpoint in `sms::estimate`, debug the explanation, and inspect borrowed
   `&str`, `Encoding`, character widths, and the segment-packing loop.
4. Inspect `AuditRule` implementations in `src/audit.rs`, then run the Unicode and
   template tests from the gutter. Add a rule by implementing the trait and registering it.
5. Start Java, generate traffic with Pulse, then run **Java history audit**. Use its
   submission IDs to trace the same notification through Java history and the report.
6. Refresh Java history in Insights for the existing traffic analysis. Use the JSON
   handoff above when implementing the additional audit overlay.

Configured push-length policies and demo secret-pattern rules are future extensions.
No security scanning or secret-detection claims are made by this first slice.

## Verify

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --release --locked
# Optional: Java must be running and idle; this script makes only GET requests:
python3 scripts/verify_live.py http://localhost:8080
```

Tests cover Unicode and GSM boundaries, template parsing, coverage counts, the
packaged CLI, cursor encoding, malformed/duplicate history, response limits, HTTP
errors, explain detail lookup, and preserving output after failures. HTTP tests
need permission to bind loopback sockets. The live script compares all submission
IDs and metadata with Java history, checks explain, and verifies unchanged statistics.

| File | Purpose |
| --- | --- |
| `src/main.rs` | CLI arguments, output, explanations, and error exit codes |
| `src/history.rs` | Read-only HTTP adapter and offline details |
| `src/model.rs` | Borrowed content access and finding enums |
| `src/audit.rs` | Independent rule trait, implementations, and versioned report |
| `src/sms.rs` | GSM-7/UTF-16 measurement and segment packing |
| `fixtures/demo-history.json` | Deterministic offline demo |
| `tests/cli.rs` | Executable-level offline and HTTP checks |
