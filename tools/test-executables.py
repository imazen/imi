#!/usr/bin/env python3
"""Read Cargo JSON messages and emit test executable paths, including custom targets.

Compiler diagnostics stay visible on stderr. The caller must use pipefail so
a failed Cargo build cannot be mistaken for a successful artifact discovery.
"""

import json
import sys


def main():
    seen = set()
    for line in sys.stdin:
        message = json.loads(line)
        if message.get("reason") == "compiler-message":
            rendered = message.get("message", {}).get("rendered")
            if rendered:
                print(rendered, end="", file=sys.stderr)
        if (
            message.get("reason") == "compiler-artifact"
            and message.get("profile", {}).get("test")
            and (path := message.get("executable"))
            and path not in seen
        ):
            seen.add(path)
            print(path)
    if not seen:
        sys.exit("Cargo emitted no test executables")


if __name__ == "__main__":
    main()
