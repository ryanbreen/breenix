# Technical Implementation Validation

Spawn a fresh-context agent to validate implementation and test quality.

**Arguments:** $ARGUMENTS

---

Use the Task tool to spawn an agent:

```
subagent_type: general-purpose
model: opus
prompt: |
  Load and follow the collaboration:technical-implementation-validation skill.

  Context: $ARGUMENTS

  Report: spec-to-test mapping, gaming patterns detected, scores (technical accuracy, intellectual honesty), categorized issues, recommendation (APPROVE/REVISE/REJECT).
```

Report validation results back to me.
