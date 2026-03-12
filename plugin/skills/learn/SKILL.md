---
name: learn
description: Analyze session observations and extract learned patterns into rules
---

# Learn — Extract Patterns from Observations

You are analyzing tool-use observations collected by the Quill widget to identify behavioral patterns that should become persistent rules.

## Step 1: Load Configuration

Run this to get the widget URL and auth secret:

```bash
python3 -c "
import json, os
config_path = os.path.expanduser('~/.config/quill/config.json')
with open(config_path) as f:
    c = json.load(f)
print(c.get('url', ''))
print(c.get('secret', ''))
"
```

Store the first line as URL and the second as SECRET.

## Step 2: Fetch Recent Observations

```bash
curl -s -H "Authorization: Bearer $SECRET" "$URL/api/v1/learning/observations?limit=200"
```

If this returns empty or fails, report "No observations available" and stop.

## Step 3: Read Existing Rules

Use Glob to find all existing rule files:
- `~/.claude/rules/**/*.md`
- `~/.claude/CLAUDE.md`
- `~/.claude/rules/learned/*.md`

Read each file to understand what rules already exist. This is critical for deduplication.

## Step 4: Analyze Patterns

Look for these patterns in the observations:
- **User corrections**: Tool calls that were undone or immediately followed by different approaches
- **Error → fix sequences**: Tool failures followed by successful corrections
- **Repeated tool workflows**: Consistent sequences of tool calls across sessions
- **Consistent preferences**: Repeated choices about style, tools, or approaches

## Step 5: Generate Rules

For each identified pattern that does NOT duplicate an existing rule:

1. Choose a descriptive kebab-case filename (e.g., `prefer-grep-over-bash-grep`)
2. Assign a domain category (e.g., `coding-style`, `tool-usage`, `testing`, `external-libs`)
3. Rate confidence from 0.0 to 1.0 based on how many observations support the pattern
4. Write the rule file to `~/.claude/rules/learned/<name>.md` with this format:

```markdown
# <Pattern Name>

**Learned:** <YYYY-MM-DD>  |  **Confidence:** <0.0-1.0>  |  **Observations:** <count>

<Clear, actionable rule description: what to do and when>
```

## Step 6: Report Results to Widget

For each new rule created, POST metadata:
```bash
curl -s -X POST -H "Authorization: Bearer $SECRET" -H "Content-Type: application/json" \
  -d '{"name":"<name>","domain":"<domain>","confidence":<confidence>,"observation_count":<count>,"file_path":"<full_path>"}' \
  "$URL/api/v1/learning/rules"
```

Then POST the run summary:
```bash
curl -s -X POST -H "Authorization: Bearer $SECRET" -H "Content-Type: application/json" \
  -d '{"trigger_mode":"on-demand","observations_analyzed":<count>,"rules_created":<count>,"rules_updated":0,"status":"completed"}' \
  "$URL/api/v1/learning/runs"
```

## Important Guidelines

- Create 0-5 rules maximum per run. Quality over quantity.
- Skip patterns that are already covered by existing rules (check CLAUDE.md and rules/ carefully)
- Keep rule descriptions concise and actionable (2-4 sentences)
- If no meaningful patterns are found, say so — don't force weak patterns
- Never create rules that contradict existing CLAUDE.md instructions
