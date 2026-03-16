# RULES.md - NexusGate

## Enforced rules (AstraGraph policy: nexusgate-default)

- All outbound LLM calls MUST include a workflow_id header.
- Budget checks are mandatory; no call may bypass the BudgetEnforcer path.
- API keys must be stored as hashes only; raw keys are never logged.
- Provider fallback is allowed on provider saturation errors only.
- MCP tool call responses must carry a trace identifier header.

## Human review required (PR, not direct commit)

- Any change to budget limits.
- Adding a new provider in the routing catalog.
- Changes to auth or budget-sensitive code paths.

## Auto-blocked (AstraGraph fail-closed)

- Calls without workflow_id.
- Calls exceeding configured budget limits.
- Any raw key logging attempt.
