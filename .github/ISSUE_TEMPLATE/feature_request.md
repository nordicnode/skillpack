name: Feature request
description: Suggest a new ecosystem target, detection rule, or capability for skillpack.
labels: ["enhancement"]
body:
  - type: markdown
    attributes:
      value: |
        Thanks for the idea! Before filing, check [the docs](https://github.com/nordicnode/skillpack/blob/main/docs/reference.md) and
        [existing issues](https://github.com/nordicnode/skillpack/issues) — your request may already be tracked.
  - type: textarea
    id: problem
    attributes:
      label: What do you want to do that skillpack can't yet?
      description: The scenario, concretely. What command or project type did you try, and what happened?
      placeholder: |
        I maintain a Python CLI and want skillpack to generate guidance for <ecosystem X>...
    validations:
      required: true
  - type: textarea
    id: proposal
    attributes:
      label: Proposed behavior
      description: What should `skillpack init` / `skillpack verify` do in this scenario?
    validations:
      required: true
  - type: textarea
    id: alternatives
    attributes:
      label: Alternatives you considered
      description: Workarounds or similar tools you've tried.
