#!/usr/bin/env bash
# Gate subagent dispatch, and record what the harness actually calls the tool.
#
# The delegate skill says when handing work to a subagent is appropriate. A skill is guidance the
# model can reason past; this is the harness refusing. Both exist on purpose — the skill explains,
# the hook enforces.
#
# Reads the PreToolUse payload on stdin and answers with a permission decision:
#   scout  -> allow. Read-only by its own brief, and its tool list has no Edit or Write, so the
#             worst case is wasted tokens.
#   others -> ask. `implementer` writes to the tree, and an agent type that does not exist yet is
#             unknown rather than safe, which is the same rule the rest of this repository takes.
#
# Every dispatch is appended to dispatch.log beside this script. That log is also how the question
# "is this tool called Task or Agent" gets answered by measurement instead of by guessing — the
# matcher in settings.json names both.
set -uo pipefail

here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
payload=$(cat)

field() { printf '%s' "$payload" | jq -r "$1 // \"\"" 2>/dev/null; }

tool=$(field '.tool_name')
agent=$(field '.tool_input.subagent_type')
desc=$(field '.tool_input.description')

printf '%s\t%s\t%s\t%s\n' "$(date -Is)" "${tool:-?}" "${agent:-none}" "${desc:-}" \
    >> "$here/dispatch.log" 2>/dev/null || true

decide() {
    jq -cn --arg d "$1" --arg r "$2" \
        '{hookSpecificOutput:{hookEventName:"PreToolUse",permissionDecision:$d,permissionDecisionReason:$r}}'
}

case "$agent" in
    scout)
        decide allow "scout is read-only: no Edit, no Write, and its brief forbids writing." ;;
    implementer)
        decide ask "implementer writes to this working tree. Confirm the brief is bounded and the scope named." ;;
    *)
        decide ask "Unrecognised subagent type '${agent:-none}' — unknown is not the same as safe." ;;
esac
