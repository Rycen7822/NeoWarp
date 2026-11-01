---
name: gui-reproduce-bug-report-local
description: Reproduce NeoWarp desktop GUI, terminal, basic-editor or settings defects with the intended build and isolated state. Use when investigating a reported runtime or visual issue.
---

# NeoWarp bug reproduction

This is a self-contained repository guide. Read the issue's steps and evidence,
then inspect the relevant source before choosing a reproduction method.

## Match the reported behavior

- Establish the NeoWarp build/revision, OS, shell and affected surface from
  available evidence. Test the reporter's OS when the suspected cause is
  OS-specific; a Linux result cannot verify a Windows IME or ConPTY defect.
- Use the corresponding NeoWarp artifact or a justified local source build.
  Record any version/platform substitution. Do not install upstream Warp or
  copy missing upstream skills as a prerequisite.
- Accounts, AI, cloud sync, built-in Notebook preview and LSP are outside the
  current product. Explain that boundary for a report about a retired feature;
  do not restore it or seek credentials to reproduce it.

## Reproduce narrowly

1. Prefer an existing focused test or script. Add a temporary probe only when
   it resolves a specific evidence gap; keep it in `.work/`.
2. For a GUI instance, use a new absolute `NEOWARP_DATA_DIR` under `.work/`.
   This isolates app data, not shell history or HOME. Preserve original Warp
   data, existing NeoWarp sessions and the user's WSL environment.
3. Follow the exact reported steps, then only variations supported by a
   concrete hypothesis. Verify actual output, state or inspected images.
4. If desktop control is necessary, follow root instructions for pausing the
   Goal and the permitted screen; do not start by exploratory clicking.

Report the producer, setup, steps, expected/observed behavior, exit results and
evidence paths. Separate a reproduced defect from an environment failure or
an unavailable check. Keep failed observations; repeated passing attempts do
not erase an intermittent failure. No cloud-agent service is required.
