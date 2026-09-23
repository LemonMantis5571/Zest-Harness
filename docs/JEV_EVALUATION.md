# Calibrating Jev checks

Keep **Check plans and changes with Jev** off by default until a labeled set of
Zest plans and changes has been measured. The current 0.90 display threshold
is provisional. Label each named check independently: a plan can have complete
coverage but conflict with a constraint; a diff can align with the task but
lack evidence for a completion claim.

Collect a mix of clear work, missing requirements, constraint conflicts,
unrelated edits, and unsupported completion claims. Include difficult negative
cases. Export the stored `checks`, `usage`, and measured request latency from
each review, and add an expert `expectedConcern` boolean to each check. Keep
the evidence and labels private if they contain project material.

The evaluator accepts a JSON array in this shape:

```json
[
  {
    "targetId": "example-plan",
    "latencyMs": 420,
    "usage": { "cost": 0.0002 },
    "checks": [
      { "id": "coverage", "choice": "concern", "probability": 0.94, "expectedConcern": true },
      { "id": "constraints", "choice": "clear", "probability": 0.93, "expectedConcern": false }
    ]
  }
]
```

Run `node scripts/evaluate-jev.mjs labeled-reviews.json`. Compare false alerts,
missed issues, precision, recall, latency, and BYOK cost at several thresholds.
Choose a threshold from this data before a broad release. Jev's choice is a
signal; have a person or normal model investigate surfaced cases.
