---
name: gui-onboarding-verification-skill
description: Verify NeoWarp GUI first startup and local onboarding using the intended NeoWarp build and isolated application data. Use for first-run terminal or Essentials verification.
---

# NeoWarp first-start verification

Verify the actual NeoWarp artifact and its local first-run experience. The
current product has no account, login, model selection or cloud onboarding.

## Prepare the intended build

- Record the artifact's source revision, OS, architecture, features and build
  result. Use an existing suitable NeoWarp build or build from this repository
  with the root environment. An upstream Warp download is not a substitute.
- Set `NEOWARP_DATA_DIR` to a new absolute directory inside the task's `.work/`
  area; `crates/warp_core/src/paths.rs` owns this override. Preserve existing
  NeoWarp and original Warp profiles, keychains and directories.
- This override isolates application data, not the shell's HOME, dotfiles or
  history. Reuse the project's private test shell/HOME setup when interaction
  is needed, or state the remaining shared boundary before proceeding.
- Prefer existing scripted checks. For user-desktop interaction, follow the
  root Goal-pause and screen-selection rules. Do not spawn cloud agents by
  default or install a different product to obtain a window.

## Verify the local flow

1. Start the intended GUI and confirm that it reaches a usable terminal without
   credentials or a network-dependent onboarding step.
2. For the requested first-run surface, inspect visible text, controls and
   layout. NeoWarp Essentials should provide local terminal tips; account,
   commercial setup and original Docs/Slack/Feedback links are retired.
3. When terminal interaction is in scope, execute a harmless marker command
   and check its actual output. A window or a successful build alone does not
   establish terminal usability.
4. Capture only the screenshots or logs needed to substantiate the requested
   claim. Close the test instance normally and keep existing user sessions.

Record the artifact, isolated directory, actions, observed results, evidence
paths and any blocked checks in `.work/`. Do not report an unexecuted step as
passed or turn one first-start check into an exhaustive onboarding matrix.
