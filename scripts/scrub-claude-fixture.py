"""Scrub a captured Claude stream-json file for committing as a test fixture.
Usage: python3 -I scripts/scrub-claude-fixture.py RAW.jsonl OUT.jsonl "$USER"

Keeps every line and the protocol fields the parser reads; replaces
machine- and user-specific values with neutral placeholders.
"""
import json, re, sys

PROJECT = "/home/user/project"
DROP_KEYS = {"credential_expires_at", "memory_paths", "scratchpad_path",
             "messaging_socket_path", "original_cwd", "additional_directories"}
LIST_PLACEHOLDERS = {
    "tools": ["Bash", "Edit", "Glob", "Grep", "Read", "Write"],
    "mcp_servers": [], "slash_commands": ["review"], "agents": ["general-purpose"],
    "skills": [], "plugins": [], "capabilities": [],
}

def scrub(obj):
    if isinstance(obj, dict):
        out = {}
        for k, v in obj.items():
            if k in DROP_KEYS:
                continue
            if k in LIST_PLACEHOLDERS:
                out[k] = LIST_PLACEHOLDERS[k]
            elif k in ("output", "stdout") and isinstance(v, str) and "hookSpecificOutput" in v:
                out[k] = "{\"hookSpecificOutput\":{\"hookEventName\":\"SessionStart\",\"additionalContext\":\"(scrubbed)\"}}"
            elif k == "cwd":
                out[k] = PROJECT
            else:
                out[k] = scrub(v)
        return out
    if isinstance(obj, list):
        return [scrub(v) for v in obj]
    if isinstance(obj, str):
        obj = re.sub(r"/tmp/claude-\d+/[^\s\"']*", PROJECT, obj)
        obj = re.sub(r"/home/[A-Za-z0-9_.-]+", "/home/user", obj)
        obj = re.sub(r"\b%s\b" % re.escape(USER), "user", obj)
        return obj
    return obj

USER = sys.argv[3]
with open(sys.argv[1]) as src, open(sys.argv[2], "w") as dst:
    for line in src:
        if line.strip():
            dst.write(json.dumps(scrub(json.loads(line)), separators=(",", ":")) + "\n")
