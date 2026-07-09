name: Bug report
description: Report something `skillpack` gets wrong (a false verify pass, a missed detection, a broken template, a crash).
labels: ["bug"]
body:
  - type: markdown
    attributes:
      value: |
        Thanks for taking the time to file a bug. The more of this you fill in, the
        faster we can reproduce it.
  - type: textarea
    id: what-happened
    attributes:
      label: What happened?
      description: The command you ran, what you expected, and what you got instead. Paste the full `--debug` output if `skillpack` misbehaved — it prints every subprocess call.
      placeholder: |
        $ skillpack init --non-interactive
        ...
        Expected: verify OK, three files written.
        Got: verify failed on ... 
    validations:
      required: true
  - type: dropdown
    id: ecosystem
    attributes:
      label: Ecosystem
      description: Which project type were you running against?
      options:
        - Rust
        - Node / npm
        - Python
        - Go
        - Ruby
        - Pure library (no CLI)
        - Other / not sure
    validations:
      required: true
  - type: input
    id: skillpack-version
    attributes:
      label: skillpack version
      description: Output of `skillpack --version` (or the git commit if built from source).
      placeholder: "0.1.0"
    validations:
      required: true
  - type: input
    id: os
    attributes:
      label: OS / runtime versions
      description: OS, and the version of the toolchain your CLI needs (e.g. node 22, python 3.14).
      placeholder: "Ubuntu 24.04, node v22, ruby 3.3"
    validations:
      required: true
