#!/usr/bin/env python3
"""Read-only smoke check against an otherwise idle Java NotifyHub instance."""

import argparse
import json
from pathlib import Path
import subprocess
import tempfile
from urllib.parse import urlencode
from urllib.request import urlopen


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("url", nargs="?", default="http://localhost:8080")
    parser.add_argument("--guard", default="target/release/guard")
    args = parser.parse_args()
    base = args.url.rstrip("/")

    def get(path):
        with urlopen(base + path, timeout=15) as response:
            return json.load(response)

    before = get("/api/stats")
    summaries = {}
    seen_cursors = set()
    cursor = None
    while True:
        query = {"limit": 200}
        if cursor is not None:
            query["cursor"] = cursor
        page = get("/api/submissions?" + urlencode(query))
        for item in page["items"]:
            identifier = item["submissionId"]
            assert identifier not in summaries, "History changed during verification"
            summaries[identifier] = item
        cursor = page["nextCursor"]
        if cursor is None:
            break
        assert cursor not in seen_cursors, "Repeated Java cursor"
        seen_cursors.add(cursor)

    with tempfile.TemporaryDirectory(prefix="guard-live-") as directory:
        report_path = Path(directory) / "audit.json"
        subprocess.run(
            [args.guard, "audit", "--url", base, "--output", str(report_path)],
            check=True,
        )
        report = json.loads(report_path.read_text())
        audited = {row["submission_id"]: row for row in report["submissions"]}
        assert report["schema_version"] == 1
        assert set(audited) == set(summaries), "Guard and Java submission IDs differ"
        assert report["summary"]["submissions"] == before["total"]["total"]
        assert len(audited) == len(report["submissions"]), "Guard counted a submission twice"
        for identifier, summary in summaries.items():
            entry = audited[identifier]
            assert entry["resolved_channel"] == summary["resolvedChannel"]
            assert entry["outcome"] == summary["outcome"]
            assert entry["submitted_at"] == summary["submittedAt"]
            assert all(finding["submission_id"] == identifier for finding in entry["findings"])
            assert "payload" not in entry
        assert report["summary"]["estimated_sms_segments"] == sum(
            row["sms"]["segments"] for row in audited.values() if row["sms"] is not None
        )
        if audited:
            identifier = next(iter(audited))
            explanation = subprocess.run(
                [args.guard, "explain", "--url", base, "--submission", identifier],
                check=True, capture_output=True, text=True,
            )
            assert identifier in explanation.stdout
        assert before == get("/api/stats"), "Java history changed; rerun on an idle instance"
        print(json.dumps({"verified": True, "summary": report["summary"], "java_stats_unchanged": True}, indent=2))


if __name__ == "__main__":
    main()
