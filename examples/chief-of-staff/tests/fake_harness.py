"""A stand-in for the `claude` and `codex` CLIs that the scout runs.

Installed on PATH under both names by the tests. It logs every invocation
and answers from ``COS_FAKE_HARNESS_RULES``: the first rule whose regex
matches the prompt supplies the text.
"""

import json
import os
import re
import sys
from pathlib import Path

name = Path(sys.argv[0]).name
argv = sys.argv[1:]
prompt = sys.stdin.read()
with open(os.environ["COS_FAKE_HARNESS_LOG"], "a", encoding="utf-8") as log:
    log.write(json.dumps({"cli": name, "argv": argv, "prompt": prompt}) + "\n")
rules = json.loads(Path(os.environ["COS_FAKE_HARNESS_RULES"]).read_text())
text = next((rule["text"] for rule in rules if re.search(rule["match"], prompt)), "")
if name == "codex":
    Path(argv[argv.index("-o") + 1]).write_text(text)
else:
    print(text)
