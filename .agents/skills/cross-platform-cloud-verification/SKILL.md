---
name: cross-platform-cloud-verification
description: Choose focused local and GitHub Actions checks for NeoWarp changes with material OS or architecture differences. Use after the affected behavior and local checks are understood.
---

# NeoWarp cross-platform verification

Use the smallest set of platforms that can expose a concrete risk in the change.
The existing GitHub Actions workflow is `.github/workflows/ci.yml`; commands,
features, runner platforms and exclusions must come from its current contents.

## Local gate and platform selection

1. Identify the affected consumer and acceptance criterion. Run the nearest
   existing check first, using the root build environment and resource limits.
2. Select another OS for OS-specific paths, processes, shells, GUI integration,
   packaging or native APIs. Select another architecture only for a relevant
   architecture gate, ABI/native dependency or reported defect.
3. Reuse existing local/WSL infrastructure where applicable. A WSL Linux result
   is Linux evidence, and does not establish Windows or macOS behavior.
4. Fix deterministic local failures before spending remote compute. Do not
   expand the platform matrix merely because more runners are available.

## CI and evidence

- Consult the NeoWarp remote, rather than the upstream Warp repository.
  Associate each observed run with its tested revision, platform, command and
  actual result. A green pushed revision does not cover newer local commits.
- Respect the existing push cadence. Checking CI does not itself authorize
  an early push, workflow dispatch, runner installation or a new cloud service.
- This skill has no Oz CLI, Warp account or cloud-agent prerequisite. Use
  additional runners or delegation only when the task calls for them and the
  required tools and authorization are available.
- Report failed checks and relevant unavailable platforms. Separate source
  inspection, compilation, executed tests and actual runtime observations;
  preserve the producer when reusing earlier evidence.

Keep one compact result per selected platform and stop when the current
acceptance criterion has sufficient evidence. Follow root instructions for
temporary artifacts, desktop control and preserving the user's running WSL.
