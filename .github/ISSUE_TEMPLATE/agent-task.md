---
name: Agent task
description: A bounded task suitable for an autonomous coding agent
labels: [agent-task]
body:
  - type: textarea
    id: goal
    attributes:
      label: Goal
      description: One concrete outcome, including the issue it implements or closes.
    validations:
      required: true
  - type: textarea
    id: scope
    attributes:
      label: Scope and non-goals
      description: Files/crates in scope; explicitly list what must not change.
    validations:
      required: true
  - type: textarea
    id: acceptance
    attributes:
      label: Acceptance criteria
      description: Checklist of observable behavior, tests, docs and evidence.
    validations:
      required: true
  - type: textarea
    id: verification
    attributes:
      label: Verification
      description: Commands or workflows that must pass (gates, golden, fuzz, soak, hardware).
    validations:
      required: true
  - type: textarea
    id: blockers
    attributes:
      label: External dependencies/blockers
      description: Hardware, credentials, approvals or upstream work the agent cannot provide.
  - type: input
    id: branch
    attributes:
      label: Suggested branch
      placeholder: feat/<area>-<slug>
